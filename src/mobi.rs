//! Convert legacy MOBI markup into a local, reflowable EPUB without subprocesses.
use crate::mobi_decode::{self, DecodedMobi};
use anyhow::{Context, Result, ensure};
use kuchiki::{NodeData, NodeRef, traits::TendrilSink};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::File,
    io::Write,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
use zip::{ZipWriter, write::SimpleFileOptions};

const MAX_CHAPTER: usize = 7 * 1024 * 1024;
const MAX_OUTPUT: usize = 128 * 1024 * 1024;

fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    ensure!(!cancel.load(Ordering::Acquire), "MOBI import cancelled");
    Ok(())
}

fn xml(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' | '\n' | '\r' => out.push(ch),
            ch if ch >= ' ' && ch != '\u{fffe}' && ch != '\u{ffff}' => out.push(ch),
            _ => {}
        }
    }
    out
}

fn text(bytes: &[u8], encoding: u32) -> Result<String> {
    Ok(match encoding {
        65001 => std::str::from_utf8(bytes)
            .context("MOBI text is not valid UTF-8")?
            .to_owned(),
        1252 => encoding_rs::WINDOWS_1252
            .decode_without_bom_handling(bytes)
            .0
            .into_owned(),
        _ => anyhow::bail!("Unsupported MOBI text encoding {encoding}"),
    })
}

fn validate_tree(document: &NodeRef, cancel: &AtomicBool) -> Result<()> {
    let mut stack = vec![(document.clone(), 0)];
    let mut count = 0;
    while let Some((node, depth)) = stack.pop() {
        count += 1;
        ensure!(
            depth <= 128 && count <= 250_000,
            "MOBI markup is too deeply nested or complex"
        );
        if count % 256 == 0 {
            check_cancel(cancel)?;
        }
        stack.extend(node.children().map(|child| (child, depth + 1)));
    }
    Ok(())
}

// HTML5 ignores the self-closing slash on MOBI's unknown pagebreak element.
// Close it explicitly so hundreds of sections do not become nested containers.
fn repair_breaks(markup: &str) -> String {
    let bytes = markup.as_bytes();
    let mut result = String::with_capacity(markup.len());
    let mut copied = 0;
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] != b'<' {
            at += 1;
            continue;
        }
        let name_end = bytes[at + 1..]
            .iter()
            .position(|b| b.is_ascii_whitespace() || matches!(b, b'/' | b'>'))
            .map(|n| at + 1 + n)
            .unwrap_or(bytes.len());
        let name = &markup[at + 1..name_end];
        if !name.eq_ignore_ascii_case("mbp:pagebreak") && !name.eq_ignore_ascii_case("pagebreak") {
            at += 1;
            continue;
        }
        let mut end = name_end;
        let mut quote = None;
        while end < bytes.len() {
            let byte = bytes[end];
            if quote == Some(byte) {
                quote = None;
            } else if quote.is_none() {
                if matches!(byte, b'\'' | b'"') {
                    quote = Some(byte);
                } else if byte == b'>' {
                    break;
                }
            }
            end += 1;
        }
        if end == bytes.len() {
            break;
        }
        result.push_str(&markup[copied..end + 1]);
        result.push_str(&format!("</{name}>"));
        copied = end + 1;
        at = copied;
    }
    result.push_str(&markup[copied..]);
    result
}

// filepos is measured in original bytes, before character decoding or HTML repair.
fn anchored_markup(book: &DecodedMobi, cancel: &AtomicBool) -> Result<String> {
    let markup = text(&book.text, book.encoding)?;
    let parsed = kuchiki::parse_html().one(repair_breaks(&markup));
    validate_tree(&parsed, cancel)?;
    let mut positions = BTreeSet::new();
    for node in parsed.descendants() {
        if let Some(element) = node.as_element()
            && let Some(position) = element
                .attributes
                .borrow()
                .get("filepos")
                .and_then(|p| p.parse::<usize>().ok())
            && position < book.text.len()
        {
            positions.insert(position);
        }
    }
    ensure!(
        positions.len() <= 20_000,
        "MOBI has too many internal link targets"
    );
    if positions.is_empty() {
        return text(&book.text, book.encoding);
    }
    let mut anchors: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut inside_tag = false;
    let mut tag_start = 0;
    let mut quote = None;
    let mut targets = positions.iter().copied().peekable();
    for (index, &byte) in book.text.iter().enumerate() {
        if index % 65536 == 0 {
            check_cancel(cancel)?;
        }
        if targets.peek() == Some(&index) {
            targets.next();
            let mut boundary = if inside_tag { tag_start } else { index };
            if !inside_tag {
                // A byte target inside an entity must not change its visible text.
                let start = index.saturating_sub(32);
                if let Some(distance) = book.text[start..index].iter().rposition(|&b| b == b'&') {
                    let entity = start + distance;
                    let end = (entity + 33).min(book.text.len());
                    if let Some(length) = book.text[entity + 1..end].iter().position(|&b| b == b';')
                    {
                        let semicolon = entity + 1 + length;
                        if index <= semicolon
                            && book.text[entity + 1..semicolon]
                                .iter()
                                .all(|b| b.is_ascii_alphanumeric() || *b == b'#')
                        {
                            boundary = entity;
                        }
                    }
                }
            }
            if book.encoding == 65001 {
                while boundary > 0 && book.text[boundary] & 0xc0 == 0x80 {
                    boundary -= 1;
                }
            }
            // A link to a break denotes the following section, not the empty
            // tail of the preceding one (which the reader would discard).
            if book.text[boundary] == b'<' {
                let tail = &book.text[boundary + 1..];
                let end = tail
                    .iter()
                    .position(|b| b.is_ascii_whitespace() || matches!(b, b'/' | b'>'))
                    .unwrap_or(tail.len());
                let name = &tail[..end];
                if name.eq_ignore_ascii_case(b"mbp:pagebreak")
                    || name.eq_ignore_ascii_case(b"pagebreak")
                {
                    let mut quoted = None;
                    for (offset, &byte) in tail.iter().enumerate().skip(end) {
                        if quoted == Some(byte) {
                            quoted = None;
                        } else if quoted.is_none() {
                            if matches!(byte, b'\'' | b'"') {
                                quoted = Some(byte);
                            } else if byte == b'>' {
                                boundary += offset + 2;
                                break;
                            }
                        }
                    }
                }
            }
            anchors.entry(boundary).or_default().push(index);
        }
        if inside_tag {
            if quote == Some(byte) {
                quote = None;
            } else if quote.is_none() {
                if byte == b'\'' || byte == b'"' {
                    quote = Some(byte);
                } else if byte == b'>' {
                    inside_tag = false;
                }
            }
        } else if byte == b'<'
            && book.text.get(index + 1).is_some_and(|next| {
                next.is_ascii_alphabetic() || matches!(next, b'/' | b'!' | b'?')
            })
        {
            inside_tag = true;
            tag_start = index;
        }
    }
    let mut bytes = Vec::with_capacity(book.text.len());
    let mut previous = 0;
    for (at, targets) in anchors {
        bytes.extend_from_slice(&book.text[previous..at]);
        for position in targets {
            bytes.extend_from_slice(format!("<span id=\"mobi-pos-{position}\"></span>").as_bytes());
        }
        previous = at;
    }
    bytes.extend_from_slice(&book.text[previous..]);
    text(&bytes, book.encoding)
}

enum Part {
    Markup(String),
    Link(String),
}
#[derive(Default)]
struct Chapter {
    parts: Vec<Part>,
    title: String,
    visible: bool,
    bytes: usize,
}
struct Markup<'a> {
    chapters: Vec<Chapter>,
    anchors: HashMap<String, usize>,
    open: Vec<String>,
    images: HashMap<usize, String>,
    bytes: usize,
    cancel: &'a AtomicBool,
}
impl<'a> Markup<'a> {
    fn new(book: &DecodedMobi, cancel: &'a AtomicBool) -> Self {
        Self {
            chapters: vec![Chapter::default()],
            anchors: HashMap::new(),
            open: Vec::new(),
            images: book
                .images
                .iter()
                .map(|image| {
                    (
                        image.recindex,
                        format!("images/{}.{}", image.recindex, image.extension),
                    )
                })
                .collect(),
            bytes: 0,
            cancel,
        }
    }
    fn current(&mut self) -> &mut Chapter {
        self.chapters.last_mut().unwrap()
    }
    fn push(&mut self, markup: String) -> Result<()> {
        self.bytes += markup.len();
        let total = self.bytes;
        let chapter = self.current();
        chapter.bytes += markup.len();
        ensure!(
            chapter.bytes < MAX_CHAPTER && total < MAX_OUTPUT,
            "Converted MOBI markup exceeds the reader's size limit"
        );
        chapter.parts.push(Part::Markup(markup));
        Ok(())
    }
    fn pagebreak(&mut self) -> Result<()> {
        if !self.current().visible {
            return Ok(());
        }
        let open = self.open.clone();
        for tag in open.iter().rev() {
            self.push(format!("</{tag}>"))?;
        }
        ensure!(self.chapters.len() < 10_000, "MOBI has too many chapters");
        self.chapters.push(Chapter::default());
        for tag in open {
            self.push(format!("<{tag}>"))?;
        }
        Ok(())
    }
    fn visit(&mut self, node: &NodeRef) -> Result<()> {
        check_cancel(self.cancel)?;
        match node.data() {
            NodeData::Text(value) => {
                let value = value.borrow();
                if !value.trim().is_empty() {
                    self.current().visible = true;
                }
                self.push(xml(&value))?;
            }
            NodeData::Element(element) => {
                let name = element.name.local.as_ref();
                if matches!(
                    name,
                    "script"
                        | "style"
                        | "head"
                        | "iframe"
                        | "object"
                        | "embed"
                        | "svg"
                        | "math"
                        | "audio"
                        | "video"
                        | "noscript"
                ) {
                    return Ok(());
                }
                if name == "mbp:pagebreak" || name == "pagebreak" {
                    self.pagebreak()?;
                    for child in node.children() {
                        self.visit(&child)?;
                    }
                    return Ok(());
                }
                let tag = match name {
                    "html" | "body" => None,
                    "div" | "section" | "article" | "p" | "h1" | "h2" | "h3" | "h4" | "h5"
                    | "h6" | "blockquote" | "pre" | "ul" | "ol" | "li" | "dl" | "dt" | "dd"
                    | "em" | "strong" | "b" | "i" | "u" | "s" | "small" | "sub" | "sup"
                    | "code" | "span" | "a" | "table" | "thead" | "tbody" | "tfoot" | "tr"
                    | "td" | "th" | "br" | "hr" | "img" => Some(name),
                    _ => Some("span"),
                };
                let attrs = element.attributes.borrow();
                if let Some(tag) = tag {
                    if matches!(tag, "h1" | "h2" | "h3") && self.current().title.is_empty() {
                        self.current().title = node
                            .text_contents()
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ")
                            .chars()
                            .take(200)
                            .collect();
                    }
                    self.push(format!("<{tag}"))?;
                    if let Some(id) = attrs.get("id").or_else(|| attrs.get("name"))
                        && !id.is_empty()
                        && id.len() < 1024
                        && !self.anchors.contains_key(id)
                    {
                        self.anchors.insert(id.to_owned(), self.chapters.len() - 1);
                        self.push(format!(" id=\"{}\"", xml(id)))?;
                    }
                    if let Some(dir) = attrs
                        .get("dir")
                        .filter(|v| matches!(*v, "rtl" | "ltr" | "auto"))
                    {
                        self.push(format!(" dir=\"{dir}\""))?;
                    }
                    if tag == "a" {
                        let target = attrs
                            .get("filepos")
                            .and_then(|v| v.parse::<usize>().ok())
                            .map(|position| format!("mobi-pos-{position}"))
                            .or_else(|| {
                                attrs
                                    .get("href")
                                    .and_then(|href| href.strip_prefix('#'))
                                    .map(|fragment| {
                                        percent_encoding::percent_decode_str(fragment)
                                            .decode_utf8_lossy()
                                            .into_owned()
                                    })
                            });
                        if let Some(target) = target
                            && target.len() < 1024
                        {
                            self.current().parts.push(Part::Link(target));
                        }
                    }
                    if tag == "img" {
                        if let Some(source) = attrs
                            .get("recindex")
                            .and_then(|v| v.parse::<usize>().ok())
                            .and_then(|index| self.images.get(&index))
                            .cloned()
                        {
                            self.push(format!(" src=\"{source}\""))?;
                            self.current().visible = true;
                        }
                        let alt = attrs.get("alt").unwrap_or_default();
                        self.push(format!(" alt=\"{}\"", xml(alt)))?;
                        if !alt.trim().is_empty() {
                            self.current().visible = true;
                        }
                    }
                    if matches!(tag, "td" | "th") {
                        for attr in ["colspan", "rowspan"] {
                            if let Some(number) = attrs
                                .get(attr)
                                .and_then(|v| v.parse::<u16>().ok())
                                .filter(|n| (1..=100).contains(n))
                            {
                                self.push(format!(" {attr}=\"{number}\""))?;
                            }
                        }
                    }
                    if matches!(tag, "br" | "hr" | "img") {
                        self.push("/>".into())?;
                        return Ok(());
                    }
                    self.push(">".into())?;
                    self.open.push(tag.into());
                }
                drop(attrs);
                for child in node.children() {
                    self.visit(&child)?;
                }
                if let Some(tag) = tag {
                    self.open.pop();
                    self.push(format!("</{tag}>"))?;
                }
            }
            NodeData::Document(_) => {
                for child in node.children() {
                    self.visit(&child)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn finish(&mut self) -> Result<()> {
        if self.chapters.len() > 1 && !self.current().visible {
            let trailing = self.chapters.pop().unwrap();
            for chapter in self.anchors.values_mut() {
                if *chapter == self.chapters.len() {
                    *chapter -= 1;
                }
            }
            self.current().parts.extend(trailing.parts);
        }
        ensure!(
            self.chapters.iter().any(|chapter| chapter.visible),
            "MOBI has no readable text or images"
        );
        for (index, chapter) in self.chapters.iter_mut().enumerate() {
            if chapter.title.is_empty() {
                chapter.title = format!("Section {}", index + 1);
            }
        }
        Ok(())
    }
}

fn entry(zip: &mut ZipWriter<File>, name: &str, bytes: &[u8], cancel: &AtomicBool) -> Result<()> {
    check_cancel(cancel)?;
    zip.start_file(
        name,
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
    )?;
    for chunk in bytes.chunks(65536) {
        check_cancel(cancel)?;
        zip.write_all(chunk)?;
    }
    Ok(())
}

pub fn convert_to_epub(source: &Path, destination: &Path, cancel: &AtomicBool) -> Result<()> {
    let book = mobi_decode::decode(source, cancel)?;
    package(&book, destination, cancel)
}

fn package(book: &DecodedMobi, destination: &Path, cancel: &AtomicBool) -> Result<()> {
    let markup = anchored_markup(book, cancel)?;
    check_cancel(cancel)?;
    let document = kuchiki::parse_html().one(repair_breaks(&markup));
    validate_tree(&document, cancel)?;
    let mut rendered = Markup::new(book, cancel);
    let body = document
        .select_first("body")
        .ok()
        .context("MOBI has no readable body")?;
    rendered.visit(body.as_node())?;
    rendered.finish()?;
    let mut zip =
        ZipWriter::new(File::create(destination).context("Cannot create converted EPUB")?);
    entry(&mut zip, "mimetype", b"application/epub+zip", cancel)?;
    entry(&mut zip, "META-INF/container.xml", br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#, cancel)?;
    let mut manifest = String::from(
        r#"<item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>"#,
    );
    let mut spine = String::new();
    let mut nav = String::from(
        r#"<?xml version="1.0" encoding="utf-8"?><html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><head><title>Contents</title></head><body><nav epub:type="toc"><ol>"#,
    );
    for (index, chapter) in rendered.chapters.iter().enumerate() {
        check_cancel(cancel)?;
        manifest.push_str(&format!(r#"<item id="c{index}" href="chapter{index}.xhtml" media-type="application/xhtml+xml"/>"#));
        spine.push_str(&format!(r#"<itemref idref="c{index}"/>"#));
        nav.push_str(&format!(
            r#"<li><a href="chapter{index}.xhtml">{}</a></li>"#,
            xml(&chapter.title)
        ));
        let mut html = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>{}</title></head><body>"#,
            xml(&chapter.title)
        );
        for part in &chapter.parts {
            match part {
                Part::Markup(value) => html.push_str(value),
                Part::Link(target) => {
                    if let Some(index) = rendered.anchors.get(target) {
                        let mut url =
                            url::Url::parse(&format!("https://mobi.invalid/chapter{index}.xhtml"))?;
                        url.set_fragment(Some(target));
                        html.push_str(&format!(
                            " href=\"{}\"",
                            xml(url.as_str().strip_prefix("https://mobi.invalid/").unwrap())
                        ));
                    }
                }
            }
            ensure!(
                html.len() < 8 * 1024 * 1024,
                "Converted MOBI chapter exceeds 8 MiB"
            );
        }
        html.push_str("</body></html>");
        entry(
            &mut zip,
            &format!("OEBPS/chapter{index}.xhtml"),
            html.as_bytes(),
            cancel,
        )?;
    }
    nav.push_str("</ol></nav></body></html>");
    entry(&mut zip, "OEBPS/nav.xhtml", nav.as_bytes(), cancel)?;
    for image in &book.images {
        let cover = if book.cover == Some(image.recindex) {
            " properties=\"cover-image\""
        } else {
            ""
        };
        let name = &rendered.images[&image.recindex];
        manifest.push_str(&format!(
            "<item id=\"image{}\" href=\"{}\" media-type=\"{}\"{cover}/>",
            image.recindex, name, image.media_type
        ));
        entry(&mut zip, &format!("OEBPS/{name}"), &image.bytes, cancel)?;
    }
    let authors = book
        .authors
        .iter()
        .map(|author| format!("<dc:creator>{}</dc:creator>", xml(author)))
        .collect::<String>();
    let identifier = format!(
        "mobi-reader:{:x}",
        <sha2::Sha256 as sha2::Digest>::digest(&book.text)
    );
    let opf = format!(
        r#"<?xml version="1.0" encoding="utf-8"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="book-id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="book-id">{identifier}</dc:identifier><dc:title>{}</dc:title>{authors}<dc:language>und</dc:language><meta property="dcterms:modified">{}</meta></metadata><manifest>{manifest}</manifest><spine>{spine}</spine></package>"#,
        xml(&book.title),
        chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ")
    );
    entry(&mut zip, "OEBPS/content.opf", opf.as_bytes(), cancel)?;
    zip.finish()?.sync_all()?;
    check_cancel(cancel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ebook::{read_book, search_book};
    use tempfile::TempDir;

    fn assert_valid_epub_xml(path: &Path) {
        use std::io::Read;
        let mut zip = zip::ZipArchive::new(File::open(path).unwrap()).unwrap();
        assert_eq!(zip.by_index(0).unwrap().name(), "mimetype");
        assert_eq!(
            zip.by_index(0).unwrap().compression(),
            zip::CompressionMethod::Stored
        );
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index).unwrap();
            if [".xml", ".xhtml", ".opf"]
                .iter()
                .any(|suffix| entry.name().ends_with(suffix))
            {
                let mut xml = String::new();
                entry.read_to_string(&mut xml).unwrap();
                roxmltree::Document::parse(&xml)
                    .unwrap_or_else(|error| panic!("{}: {error}", entry.name()));
            }
        }
    }

    #[test]
    fn converts_metadata_cover_sections_and_filepos_links() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("original.mobi");
        let epub = temp.path().join("converted.epub");
        std::fs::write(&source, mobi_decode::fixture()).unwrap();
        convert_to_epub(&source, &epub, &AtomicBool::new(false)).unwrap();
        assert_valid_epub_xml(&epub);
        let book = read_book(&epub).unwrap();
        assert_eq!(book.metadata.title, "Native MOBI Fixture");
        assert_eq!(book.metadata.author, "Mobi Reader Test Studio");
        assert!(book.metadata.cover_data.is_some());
        assert_eq!(book.chapters.len(), 2);
        let html = book.chapters[0]
            .blocks
            .iter()
            .map(|b| b.html.as_str())
            .collect::<String>();
        assert!(html.contains("reader://chapter/1#mobi-pos-"), "{html}");
        assert!(book.chapters[1].blocks.iter().any(|block| {
            block
                .source_ids
                .iter()
                .any(|id| id.starts_with("mobi-pos-"))
        }));
        assert!(
            book.chapters
                .iter()
                .flat_map(|c| &c.blocks)
                .any(|b| b.html.contains("data:image/png;base64,"))
        );
    }

    #[test]
    fn normalizes_markup_escaping_named_links_and_nested_breaks() {
        let temp = TempDir::new().unwrap();
        let book = DecodedMobi {
            text: br##"<html><body><div id="start"><h1>A &amp; B</h1><p><a href="#second">Next</a><script>hidden-script</script></p><mbp:pagebreak/><h2 id="second">Second</h2><p onclick="bad()">hello &amp; goodbye<img src="https://example.com/a" alt="missing"/></p><p><a href="https://example.com">external</a><a href="#start">Back</a></p></div><mbp:pagebreak/></body></html>"##.to_vec(),
            encoding: 65001, title: "A & B <title>".into(), authors: vec!["An & Author".into()], images: vec![], cover: None,
        };
        let output = temp.path().join("markup.epub");
        package(&book, &output, &AtomicBool::new(false)).unwrap();
        assert_valid_epub_xml(&output);
        let result = read_book(&output).unwrap();
        assert_eq!(result.chapters.len(), 2);
        assert_eq!(result.metadata.title, book.title);
        assert_eq!(result.toc[0].title, "A & B");
        let html = result
            .chapters
            .iter()
            .flat_map(|c| &c.blocks)
            .map(|b| b.html.as_str())
            .collect::<String>();
        assert!(html.contains("reader://chapter/1#second"));
        assert!(html.contains("reader://chapter/0#start"));
        assert!(!html.contains("onclick") && !html.contains("https://example.com"));
        assert!(search_book(&result, "hello & goodbye").len() == 1);
        assert!(search_book(&result, "hidden-script").is_empty());
    }

    #[test]
    fn filepos_uses_original_windows1252_byte_offsets() {
        let temp = TempDir::new().unwrap();
        let mut content =
            b"<p>\x93quoted\x94 <a filepos=0000000000>Next</a></p><mbp:pagebreak/>".to_vec();
        let target = content.len();
        content.extend_from_slice(b"<h1>Destination</h1><p>Arrived.</p>");
        let at = content
            .windows(10)
            .position(|w| w == b"0000000000")
            .unwrap();
        content[at..at + 10].copy_from_slice(format!("{target:010}").as_bytes());
        let book = DecodedMobi {
            text: content,
            encoding: 1252,
            title: "Encoded".into(),
            authors: vec![],
            images: vec![],
            cover: None,
        };
        let output = temp.path().join("encoding.epub");
        package(&book, &output, &AtomicBool::new(false)).unwrap();
        let result = read_book(&output).unwrap();
        assert!(result.chapters[0].blocks.iter().any(|b| {
            b.html
                .contains(&format!("reader://chapter/1#mobi-pos-{target}"))
        }));
        assert!(
            result.chapters[0]
                .blocks
                .iter()
                .any(|b| b.text.contains('\u{201c}'))
        );
    }

    #[test]
    fn many_self_closing_pagebreaks_do_not_create_artificial_nesting() {
        let temp = TempDir::new().unwrap();
        let markup = (0..150)
            .map(|index| format!("<h1>Section {index}</h1><p>Text.</p><mbp:pagebreak/>"))
            .collect::<String>();
        let book = DecodedMobi {
            text: markup.into_bytes(),
            encoding: 65001,
            title: "Long book".into(),
            authors: vec![],
            images: vec![],
            cover: None,
        };
        let output = temp.path().join("long.epub");
        package(&book, &output, &AtomicBool::new(false)).unwrap();
        assert_valid_epub_xml(&output);
        assert_eq!(read_book(&output).unwrap().chapters.len(), 150);
    }

    #[test]
    fn filepos_inside_an_entity_preserves_the_character() {
        let content = b"<p>A &amp; B</p><a filepos=0000000008>Back</a>";
        let book = DecodedMobi {
            text: content.to_vec(),
            encoding: 65001,
            title: "Entities".into(),
            authors: vec![],
            images: vec![],
            cover: None,
        };
        let markup = anchored_markup(&book, &AtomicBool::new(false)).unwrap();
        let document = kuchiki::parse_html().one(markup);
        assert!(document.text_contents().contains("A & B"));
        assert!(document.select_first("#mobi-pos-8").is_ok());
    }

    #[test]
    fn filepos_at_a_pagebreak_targets_the_following_section() {
        let temp = TempDir::new().unwrap();
        let mut content = b"<p><a filepos=0000000000>Next</a></p>".to_vec();
        let position = content.len();
        content.extend_from_slice(b"<mbp:pagebreak/><h1>Next section</h1>");
        let at = content
            .windows(10)
            .position(|b| b == b"0000000000")
            .unwrap();
        content[at..at + 10].copy_from_slice(format!("{position:010}").as_bytes());
        let book = DecodedMobi {
            text: content,
            encoding: 65001,
            title: "Break target".into(),
            authors: vec![],
            images: vec![],
            cover: None,
        };
        let output = temp.path().join("break.epub");
        package(&book, &output, &AtomicBool::new(false)).unwrap();
        let result = read_book(&output).unwrap();
        let anchor = format!("mobi-pos-{position}");
        assert!(
            result.chapters[0]
                .blocks
                .iter()
                .any(|b| b.html.contains(&format!("reader://chapter/1#{anchor}")))
        );
        assert!(
            result.chapters[1]
                .blocks
                .iter()
                .any(|b| b.source_ids.contains(&anchor))
        );
    }
}
