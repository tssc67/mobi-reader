//! EPUB extraction and a deliberately small, resource-isolated reading HTML vocabulary.
use crate::model::{BookDocument, BookMetadata, Chapter, ContentBlock, SearchHit, TocEntry};
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use kuchiki::{NodeData, NodeRef, traits::TendrilSink};
use rbook::Epub;
use rbook::ebook::errors::ArchiveError;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::File,
    io::{self, Write},
    path::Path,
};
use url::Url;

const MAX_RESOURCE: usize = 8 * 1024 * 1024;
const MAX_BOOK_TEXT: usize = 128 * 1024 * 1024;
const MAX_IMAGES: usize = 64 * 1024 * 1024;

struct LimitedBytes {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for LimitedBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other(
                "Book resource exceeds the reader's size limit",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn resource(epub: &Epub, href: &str, limit: usize) -> Result<Vec<u8>> {
    let mut writer = LimitedBytes {
        bytes: Vec::new(),
        limit,
    };
    epub.copy_resource(href, &mut writer)
        .with_context(|| format!("Cannot read book resource {href}"))?;
    Ok(writer.bytes)
}

/// Read an EPUB file. MOBI files are converted by the built-in import pipeline first.
pub fn read_book(path: &Path) -> Result<BookDocument> {
    let file = File::open(path).with_context(|| format!("Cannot open {}", path.display()))?;
    if !file.metadata()?.is_file() {
        bail!("Choose an EPUB file, not a directory");
    }
    let epub = Epub::read(file)
        .context("This file is not a readable EPUB. DRM-protected books are unsupported")?;
    let metadata = epub.metadata();
    if metadata
        .by_property("rendition:layout")
        .any(|m| m.value().trim() == "pre-paginated")
    {
        bail!("Fixed-layout EPUB books are unsupported; choose a reflowable edition");
    }
    // Font obfuscation is permitted because publisher fonts are never loaded.
    let encryption = match resource(&epub, "/META-INF/encryption.xml", 1024 * 1024) {
        Ok(xml) => Some(xml),
        Err(error) if matches!(error.downcast_ref::<ArchiveError>(), Some(ArchiveError::InvalidResource { source, .. }) if source.kind() == io::ErrorKind::NotFound) => {
            None
        }
        Err(error) => return Err(error.context("Cannot inspect EPUB encryption information")),
    };
    if let Some(xml) = encryption {
        let xml = String::from_utf8(xml).context("Invalid EPUB encryption information")?;
        let document = kuchiki::parse_html().one(xml);
        let mut encrypted_entries = 0;
        let mut allowed_methods = 0;
        for node in document.descendants() {
            if let Some(element) = node.as_element() {
                if element.name.local.as_ref().split(':').next_back() == Some("encrypteddata") {
                    encrypted_entries += 1;
                    let font_only = node
                        .descendants()
                        .find_map(|child| {
                            let element = child.as_element()?;
                            if element.name.local.as_ref().split(':').next_back()
                                != Some("cipherreference")
                            {
                                return None;
                            }
                            let attrs = element.attributes.borrow();
                            let (key, _) = resolve("/", attrs.get("uri")?)?;
                            Some(epub.manifest().iter().any(|entry| {
                                entry.kind().is_font()
                                    && resolve("/", entry.href().as_str())
                                        .is_some_and(|(path, _)| path == key)
                            }))
                        })
                        .unwrap_or(false);
                    if !font_only {
                        bail!("Encrypted or DRM-protected EPUB content is unsupported");
                    }
                }
                if element.name.local.as_ref().split(':').next_back() == Some("encryptionmethod") {
                    let attrs = element.attributes.borrow();
                    let algorithm = attrs.get("algorithm").unwrap_or_default();
                    if algorithm != "http://www.idpf.org/2008/embedding"
                        && algorithm != "http://ns.adobe.com/pdf/enc#RC"
                    {
                        bail!("Encrypted or DRM-protected EPUB books are unsupported");
                    }
                    allowed_methods += 1;
                }
            }
        }
        if encrypted_entries != allowed_methods {
            bail!("Encrypted or malformed EPUB encryption information is unsupported");
        }
    }
    let title = metadata
        .title()
        .map(|t| t.value().trim().to_owned())
        .filter(|t| !t.is_empty())
        .unwrap_or_default();
    let authors = metadata
        .creators()
        .map(|a| a.value().trim().to_owned())
        .filter(|a| !a.is_empty())
        .collect::<Vec<_>>();
    let mut images = HashMap::new();
    let mut image_bytes = 0usize;
    for image in epub.manifest().images() {
        let mime = image.media_type();
        // SVG can contain scripts, external references and embedded HTML.
        if !matches!(
            mime,
            "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/avif" | "image/bmp"
        ) {
            continue;
        }
        if let Ok(bytes) = resource(&epub, image.href().as_str(), MAX_RESOURCE) {
            image_bytes = image_bytes.saturating_add(bytes.len());
            if image_bytes > MAX_IMAGES {
                bail!("Book images exceed the reader's 64 MiB limit");
            }
            if let Some((key, _)) = resolve("/", image.href().as_str()) {
                images.insert(
                    key,
                    format!("data:{mime};base64,{}", STANDARD.encode(bytes)),
                );
            }
        }
    }
    let cover_data = epub.manifest().cover_image().and_then(|image| {
        resolve("/", image.href().as_str()).and_then(|(key, _)| images.get(&key).cloned())
    });
    let mut sources = Vec::new();
    let mut total_text = 0usize;
    for entry in epub.spine().iter() {
        if entry
            .properties()
            .has_property("rendition:layout-pre-paginated")
        {
            bail!("Fixed-layout EPUB books are unsupported; choose a reflowable edition");
        }
        let manifest = entry
            .manifest_entry()
            .context("EPUB reading order references a missing chapter")?;
        if !matches!(manifest.media_type(), "application/xhtml+xml" | "text/html") {
            bail!("Unsupported chapter type: {}", manifest.media_type());
        }
        let href = manifest.href().as_str().to_owned();
        let bytes = resource(&epub, &href, MAX_RESOURCE)?;
        total_text += bytes.len();
        if total_text > MAX_BOOK_TEXT || sources.len() >= 10000 {
            bail!("Book exceeds the reader's chapter/content limit");
        }
        sources.push((
            href,
            String::from_utf8(bytes).context("Chapter text is not valid UTF-8")?,
        ));
    }
    if sources.is_empty() {
        bail!("EPUB has no readable chapters");
    }
    let chapter_map = sources
        .iter()
        .enumerate()
        .filter_map(|(i, (href, _))| resolve("/", href).map(|(key, _)| (key, i)))
        .collect::<HashMap<_, _>>();
    let mut toc = Vec::new();
    if let Some(root) = epub.toc().contents() {
        for entry in root.flatten() {
            let destination = entry
                .href()
                .and_then(|h| resolve("/", h.as_str()))
                .or_else(|| {
                    entry.flatten().find_map(|child| {
                        child
                            .href()
                            .and_then(|h| resolve("/", h.as_str()))
                            .filter(|(key, _)| chapter_map.contains_key(key))
                    })
                });
            if let Some((key, fragment)) = destination
                && let Some(&spine_index) = chapter_map.get(&key)
            {
                toc.push(TocEntry {
                    title: entry.label().to_owned(),
                    spine_index,
                    fragment,
                    depth: entry.depth().saturating_sub(1),
                });
            }
        }
    }
    let mut chapters = Vec::new();
    for (spine_index, (href, xhtml)) in sources.into_iter().enumerate() {
        let blocks = normalize(&xhtml, &href, spine_index, &images, &chapter_map)?;
        let title = toc
            .iter()
            .find(|entry| entry.spine_index == spine_index)
            .map(|entry| entry.title.clone())
            .or_else(|| {
                blocks
                    .iter()
                    .find(|block| {
                        (1..=6).any(|level| block.html.starts_with(&format!("<h{level}")))
                    })
                    .map(|block| block.text.clone())
            })
            .unwrap_or_else(|| format!("Chapter {}", spine_index + 1));
        chapters.push(Chapter {
            title,
            spine_index,
            href,
            blocks,
        });
    }
    if chapters.iter().all(|chapter| chapter.blocks.is_empty()) {
        bail!("EPUB contains no readable text or supported images");
    }
    if toc.is_empty() {
        toc = chapters
            .iter()
            .map(|chapter| TocEntry {
                title: chapter.title.clone(),
                spine_index: chapter.spine_index,
                fragment: None,
                depth: 0,
            })
            .collect();
    }
    Ok(BookDocument {
        metadata: BookMetadata {
            title,
            author: if authors.is_empty() {
                "Unknown author".into()
            } else {
                authors.join(", ")
            },
            cover_data,
            chapter_count: chapters.len(),
        },
        toc,
        chapters,
    })
}

fn resolve(base: &str, reference: &str) -> Option<(String, Option<String>)> {
    let reference = reference.trim();
    if reference.starts_with("//")
        || reference.contains('\\')
        || reference.chars().any(char::is_control)
    {
        return None;
    }
    let root = Url::parse("https://book.invalid/").ok()?;
    let base = root.join(base).ok()?;
    let target = base.join(reference).ok()?;
    if target.scheme() != "https"
        || target.host_str() != Some("book.invalid")
        || !target.username().is_empty()
        || target.password().is_some()
    {
        return None;
    }
    // Explicit schemes are never book-local, even if their hostname matches ours.
    if Url::parse(reference).is_ok() {
        return None;
    }
    Some((
        decode_fragment(target.path()),
        target.fragment().map(decode_fragment),
    ))
}
fn decode_fragment(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| match b {
                b'0'..=b'9' => Some(b - b'0'),
                b'a'..=b'f' => Some(b - b'a' + 10),
                b'A'..=b'F' => Some(b - b'A' + 10),
                _ => None,
            };
            if let (Some(high), Some(low)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                output.push(high * 16 + low);
                i += 3;
                continue;
            }
        }
        output.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
fn compact(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn blocked(tag: &str) -> bool {
    matches!(
        tag,
        "script"
            | "style"
            | "link"
            | "meta"
            | "iframe"
            | "object"
            | "embed"
            | "form"
            | "input"
            | "button"
            | "textarea"
            | "select"
            | "svg"
            | "math"
            | "audio"
            | "video"
            | "source"
            | "canvas"
            | "noscript"
            | "template"
    )
}
fn unit(tag: &str) -> bool {
    matches!(
        tag,
        "p" | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "ul"
            | "ol"
            | "dl"
            | "blockquote"
            | "pre"
            | "table"
            | "figure"
            | "hr"
    )
}
fn allowed(tag: &str) -> bool {
    unit(tag)
        || matches!(
            tag,
            "li" | "dt"
                | "dd"
                | "em"
                | "strong"
                | "b"
                | "i"
                | "u"
                | "s"
                | "small"
                | "sub"
                | "sup"
                | "code"
                | "br"
                | "img"
                | "a"
                | "span"
                | "div"
                | "figcaption"
                | "caption"
                | "thead"
                | "tbody"
                | "tfoot"
                | "tr"
                | "td"
                | "th"
                | "q"
                | "cite"
                | "abbr"
        )
}

struct Render<'a> {
    href: &'a str,
    images: &'a HashMap<String, String>,
    chapters: &'a HashMap<String, usize>,
}
impl Render<'_> {
    fn node(&self, node: &NodeRef, ids: &mut Vec<String>, text: &mut String) -> String {
        match node.data() {
            NodeData::Text(value) => {
                let value = value.borrow();
                text.push_str(&value);
                escape(&value)
            }
            NodeData::Element(element) => {
                let tag = element.name.local.as_ref();
                if blocked(tag) {
                    return String::new();
                }
                let attrs = element.attributes.borrow();
                let mut attributes = String::new();
                if let Some(id) = attrs
                    .get("id")
                    .or_else(|| if tag == "a" { attrs.get("name") } else { None })
                    && !id.is_empty()
                {
                    ids.push(id.to_owned());
                    attributes.push_str(&format!(" id=\"{}\"", escape(id)));
                }
                if let Some(dir) = attrs
                    .get("dir")
                    .filter(|dir| matches!(*dir, "rtl" | "ltr" | "auto"))
                {
                    attributes.push_str(&format!(" dir=\"{dir}\""));
                }
                if tag == "img" {
                    let alt = attrs.get("alt").unwrap_or_default();
                    let source = attrs
                        .get("src")
                        .and_then(|src| resolve(self.href, src))
                        .and_then(|(key, _)| self.images.get(&key));
                    text.push_str(alt);
                    return source
                        .map(|source| {
                            format!(
                                "<img{attributes} src=\"{}\" alt=\"{}\">",
                                escape(source),
                                escape(alt)
                            )
                        })
                        .unwrap_or_else(|| escape(alt));
                }
                if tag == "a"
                    && let Some((key, fragment)) =
                        attrs.get("href").and_then(|href| resolve(self.href, href))
                    && let Some(index) = self.chapters.get(&key)
                {
                    let mut target = Url::parse(&format!("reader://chapter/{index}"))
                        .expect("static reader URL");
                    target.set_fragment(fragment.as_deref());
                    attributes.push_str(&format!(" href=\"{}\"", escape(target.as_str())));
                }
                if matches!(tag, "td" | "th") {
                    for name in ["colspan", "rowspan"] {
                        if let Some(number) = attrs
                            .get(name)
                            .and_then(|value| value.parse::<u16>().ok())
                            .filter(|n| *n > 0 && *n <= 100)
                        {
                            attributes.push_str(&format!(" {name}=\"{number}\""));
                        }
                    }
                }
                if tag == "br" || tag == "hr" {
                    text.push(' ');
                    return format!("<{tag}{attributes}>");
                }
                let children = node
                    .children()
                    .map(|child| self.node(&child, ids, text))
                    .collect::<String>();
                if unit(tag) || matches!(tag, "li" | "td" | "th" | "dt" | "dd" | "tr") {
                    text.push(' ');
                }
                if allowed(tag) {
                    format!("<{tag}{attributes}>{children}</{tag}>")
                } else {
                    children
                }
            }
            _ => node
                .children()
                .map(|child| self.node(&child, ids, text))
                .collect(),
        }
    }
}
fn normalize(
    xhtml: &str,
    href: &str,
    index: usize,
    images: &HashMap<String, String>,
    chapters: &HashMap<String, usize>,
) -> Result<Vec<ContentBlock>> {
    let document = kuchiki::parse_html().one(xhtml);
    let body = document
        .select_first("body")
        .map(|n| n.as_node().clone())
        .unwrap_or(document);
    // Prevent pathological markup from overflowing the recursive semantic renderer.
    let mut stack = vec![(body.clone(), 0usize)];
    let mut count = 0;
    while let Some((node, depth)) = stack.pop() {
        count += 1;
        if depth > 128 || count > 250000 {
            bail!("Chapter markup is too deeply nested or complex: {href}");
        }
        stack.extend(node.children().map(|child| (child, depth + 1)));
    }
    let renderer = Render {
        href,
        images,
        chapters,
    };
    let mut blocks = Vec::new();
    fn visit(
        node: &NodeRef,
        renderer: &Render<'_>,
        index: usize,
        ancestors: &[String],
        blocks: &mut Vec<ContentBlock>,
    ) {
        let tag = node.as_element().map(|e| e.name.local.as_ref());
        if tag.is_some_and(blocked) {
            return;
        }
        let is_unit =
            tag.is_some_and(unit) || tag == Some("img") || matches!(node.data(), NodeData::Text(_));
        if is_unit {
            let mut source_ids = ancestors.to_vec();
            let mut text = String::new();
            let html = renderer.node(node, &mut source_ids, &mut text);
            let text = compact(&text);
            if text.is_empty() && !html.contains("<img") && tag != Some("hr") {
                return;
            }
            source_ids.sort();
            source_ids.dedup();
            let digest =
                Sha256::digest(format!("{}\0{}\0{}", renderer.href, blocks.len(), html).as_bytes());
            let id = format!("c{index}-b{}-{:x}", blocks.len(), digest);
            blocks.push(ContentBlock {
                id,
                html: if tag.is_none() {
                    format!("<p>{html}</p>")
                } else {
                    html
                },
                text,
                source_ids,
            });
        } else {
            let mut ids = ancestors.to_vec();
            if let Some(id) = node
                .as_element()
                .and_then(|e| e.attributes.borrow().get("id").map(str::to_owned))
            {
                ids.push(id);
            }
            // A container with only inline descendants is a single paragraph, preserving emphasis.
            let has_units = node
                .descendants()
                .skip(1)
                .any(|n| n.as_element().is_some_and(|e| unit(e.name.local.as_ref())));
            if tag != Some("body") && !has_units && !compact(&node.text_contents()).is_empty() {
                let mut text = String::new();
                let html = renderer.node(node, &mut ids, &mut text);
                let text = compact(&text);
                if !text.is_empty() || html.contains("<img") {
                    let digest = Sha256::digest(
                        format!("{}\0{}\0{}", renderer.href, blocks.len(), html).as_bytes(),
                    );
                    ids.sort();
                    ids.dedup();
                    blocks.push(ContentBlock {
                        id: format!("c{index}-b{}-{:x}", blocks.len(), digest),
                        html: format!("<div>{html}</div>"),
                        text,
                        source_ids: ids,
                    });
                }
            } else {
                let mut pending_ids = Vec::new();
                for child in node.children() {
                    let mut child_ids = ids.clone();
                    child_ids.extend(pending_ids.iter().cloned());
                    let before = blocks.len();
                    visit(&child, renderer, index, &child_ids, blocks);
                    if blocks.len() > before {
                        pending_ids.clear();
                    } else if let Some(element) = child.as_element()
                        && !blocked(element.name.local.as_ref())
                    {
                        let attrs = element.attributes.borrow();
                        if let Some(id) = attrs
                            .get("id")
                            .or_else(|| attrs.get("name"))
                            .map(str::to_owned)
                        {
                            pending_ids.push(id);
                        }
                    }
                }
            }
        }
    }
    visit(&body, &renderer, index, &[], &mut blocks);
    Ok(blocks)
}

/// Case-insensitive Unicode search with at most one result per semantic block.
pub fn search_book(book: &BookDocument, query: &str) -> Vec<SearchHit> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for chapter in &book.chapters {
        for block in &chapter.blocks {
            let lowered = block.text.to_lowercase();
            if let Some(byte) = lowered.find(&query) {
                // Lowercasing may expand code points; map offsets in the folded string
                // back to original characters instead of slicing at folded byte positions.
                let folded_chars = lowered[..byte].chars().count();
                let original = block.text.chars().collect::<Vec<_>>();
                let mut folded = 0;
                let mut position = 0;
                for (i, character) in original.iter().enumerate() {
                    if folded >= folded_chars {
                        position = i;
                        break;
                    }
                    folded += character.to_lowercase().count();
                    position = i + 1;
                }
                let start = position.saturating_sub(45);
                let end = (position + query.chars().count() + 100).min(original.len());
                let snippet = format!(
                    "{}{}{}",
                    if start > 0 { "…" } else { "" },
                    original[start..end].iter().collect::<String>(),
                    if end < original.len() { "…" } else { "" }
                );
                hits.push(SearchHit {
                    spine_index: chapter.spine_index,
                    block_id: block.id.clone(),
                    chapter_title: chapter.title.clone(),
                    snippet,
                });
                if hits.len() >= 500 {
                    return hits;
                }
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use zip::{ZipWriter, write::SimpleFileOptions};

    fn fixture(extra_metadata: &str, encryption: Option<&str>) -> tempfile::NamedTempFile {
        fixture_with_title(Some("The Reader Test"), extra_metadata, encryption)
    }

    fn fixture_with_title(
        title: Option<&str>,
        extra_metadata: &str,
        encryption: Option<&str>,
    ) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut archive = ZipWriter::new(file.reopen().unwrap());
        let mut add = |path: &str, contents: &[u8]| {
            archive
                .start_file(
                    path,
                    SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
                )
                .unwrap();
            archive.write_all(contents).unwrap();
        };
        add("mimetype", b"application/epub+zip");
        add("META-INF/container.xml", br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="EPUB/package.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#);
        let title_xml = title
            .map(|title| format!("<dc:title>{}</dc:title>", escape(title)))
            .unwrap_or_default();
        let opf = format!(
            r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="book-id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="book-id">test-id</dc:identifier>{title_xml}<dc:creator>Jane Reader</dc:creator><dc:language>en</dc:language><meta property="dcterms:modified">2026-09-27T00:00:00Z</meta>{extra_metadata}</metadata><manifest><item id="second" href="second.xhtml" media-type="application/xhtml+xml"/><item id="first" href="text/first.xhtml" media-type="application/xhtml+xml"/><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/><item id="cover" href="images/cover.png" media-type="image/png" properties="cover-image"/></manifest><spine><itemref idref="first"/><itemref idref="second"/></spine></package>"#
        );
        add("EPUB/package.opf", opf.as_bytes());
        add("EPUB/nav.xhtml", br##"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><head><title>Contents</title></head><body><nav epub:type="toc"><ol><li><a href="text/first.xhtml#opening">First chapter</a><ol><li><a href="text/first.xhtml#detail">A detail</a></li></ol></li><li><a href="second.xhtml">Second chapter</a></li></ol></nav></body></html>"##);
        add("EPUB/text/first.xhtml", br##"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>One</title><style>body { display: none }</style></head><body><section id="opening"><h1>First chapter</h1><a id="detail"></a><p onclick="alert(1)">Read <em>quietly</em> &amp; <strong>well</strong>. <a href="../second.xhtml#end">Continue</a><a href="https://evil.invalid">External</a><script>bad()</script></p><ul><li>One</li><li>Two</li></ul><table><tr><th>Name</th><td colspan="2">Reader</td></tr></table><p><img src="../images/cover.png" alt="Cover"/><img src="file:///C:/secret.png" alt="Blocked"/></p><iframe src="https://evil.invalid"></iframe></section></body></html>"##);
        add(
            "EPUB/second.xhtml",
            "<html><body><h2 id='end'>第二章</h2><p>CAFÉ İstanbul ไทย อ่านหนังสือ</p></body></html>"
                .as_bytes(),
        );
        // A valid transparent 1x1 PNG.
        let png = STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=").unwrap();
        add("EPUB/images/cover.png", &png);
        if let Some(xml) = encryption {
            add("META-INF/encryption.xml", xml.as_bytes());
        }
        archive.finish().unwrap();
        file
    }

    fn fixture_epub2() -> tempfile::NamedTempFile {
        let source = fixture("<meta name='cover' content='cover'/>", None);
        let mut old = zip::ZipArchive::new(source.reopen().unwrap()).unwrap();
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut archive = ZipWriter::new(file.reopen().unwrap());
        for index in 0..old.len() {
            let mut entry = old.by_index(index).unwrap();
            let name = entry.name().to_owned();
            if name == "EPUB/nav.xhtml" {
                continue;
            }
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            if name == "EPUB/package.opf" {
                let opf = String::from_utf8(bytes).unwrap()
                    .replace("version=\"3.0\"", "version=\"2.0\"")
                    .replace("<meta property=\"dcterms:modified\">2026-09-27T00:00:00Z</meta>", "")
                    .replace("<item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\"/>", "<item id=\"ncx\" href=\"toc.ncx\" media-type=\"application/x-dtbncx+xml\"/>")
                    .replace(" properties=\"cover-image\"", "")
                    .replace("<spine>", "<spine toc=\"ncx\">");
                bytes = opf.into_bytes();
            }
            archive
                .start_file(
                    name,
                    SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
                )
                .unwrap();
            archive.write_all(&bytes).unwrap();
        }
        archive
            .start_file("EPUB/toc.ncx", SimpleFileOptions::default())
            .unwrap();
        archive.write_all(br##"<?xml version="1.0"?><ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1"><head><meta name="dtb:uid" content="test-id"/></head><docTitle><text>The Reader Test</text></docTitle><navMap><navPoint id="n1" playOrder="1"><navLabel><text>First chapter</text></navLabel><content src="text/first.xhtml#opening"/><navPoint id="n2" playOrder="2"><navLabel><text>A detail</text></navLabel><content src="text/first.xhtml#detail"/></navPoint></navPoint><navPoint id="n3" playOrder="3"><navLabel><text>Second chapter</text></navLabel><content src="second.xhtml#end"/></navPoint></navMap></ncx>"##).unwrap();
        archive.finish().unwrap();
        file
    }

    #[test]
    fn preserves_missing_title_for_importer_source_filename_fallback() {
        for title in [None, Some("   ")] {
            let file = fixture_with_title(title, "", None);
            let book = read_book(file.path()).unwrap();
            assert!(book.metadata.title.is_empty());
            assert_eq!(book.chapters.len(), 2);
        }
    }

    #[test]
    fn reads_epub2_ncx_navigation_and_legacy_cover() {
        let file = fixture_epub2();
        let book = read_book(file.path()).unwrap();
        assert_eq!(book.metadata.title, "The Reader Test");
        assert_eq!(book.chapters.len(), 2);
        assert!(book.metadata.cover_data.is_some());
        assert_eq!(
            book.toc.iter().map(|entry| entry.depth).collect::<Vec<_>>(),
            vec![0, 1, 0]
        );
        assert_eq!(book.toc[1].fragment.as_deref(), Some("detail"));
        assert_eq!(book.toc[2].fragment.as_deref(), Some("end"));
        assert_eq!(book.toc[2].spine_index, 1);
    }

    #[test]
    fn reads_spine_metadata_nested_toc_and_safe_semantic_html() {
        let file = fixture("", None);
        let book = read_book(file.path()).unwrap();
        assert_eq!(book.metadata.title, "The Reader Test");
        assert_eq!(book.metadata.author, "Jane Reader");
        assert_eq!(book.metadata.chapter_count, 2);
        assert_eq!(book.chapters[0].title, "First chapter");
        assert!(book.chapters[0].href.ends_with("text/first.xhtml"));
        assert_eq!(book.toc.len(), 3);
        assert_eq!(book.toc[1].depth, 1);
        assert_eq!(book.toc[1].fragment.as_deref(), Some("detail"));
        assert_eq!(book.toc[2].spine_index, 1);
        assert!(
            book.metadata
                .cover_data
                .as_ref()
                .unwrap()
                .starts_with("data:image/png;base64,")
        );
        let html = book.chapters[0]
            .blocks
            .iter()
            .map(|block| block.html.as_str())
            .collect::<String>();
        assert!(html.contains("<em>quietly</em>"));
        assert!(html.contains("<strong>well</strong>"));
        assert!(html.contains("<ul><li>One</li><li>Two</li></ul>"));
        assert!(html.contains("colspan=\"2\""));
        assert!(html.contains("reader://chapter/1#end"));
        assert!(html.contains("data:image/png;base64,"));
        for forbidden in [
            "onclick",
            "script",
            "iframe",
            "evil.invalid",
            "file:",
            "style",
            "bad()",
        ] {
            assert!(!html.contains(forbidden), "{forbidden} survived");
        }
        assert!(
            book.chapters[0]
                .blocks
                .iter()
                .any(|block| block.source_ids.contains(&"detail".to_owned()))
        );
        let again = read_book(file.path()).unwrap();
        assert_eq!(book, again);
    }

    #[test]
    fn searches_unicode_without_byte_boundary_panics() {
        let file = fixture("", None);
        let book = read_book(file.path()).unwrap();
        for query in ["café", "i̇stanbul", "ไทย", "第二"] {
            let hits = search_book(&book, query);
            assert_eq!(hits.len(), 1, "{query}");
            assert_eq!(hits[0].spine_index, 1);
            assert!(
                book.chapters[1]
                    .blocks
                    .iter()
                    .any(|b| b.id == hits[0].block_id)
            );
        }
        assert!(search_book(&book, "  ").is_empty());
    }

    #[test]
    fn rejects_bad_encrypted_and_fixed_layout_books() {
        let bad = tempfile::NamedTempFile::new().unwrap();
        assert!(
            read_book(bad.path())
                .unwrap_err()
                .to_string()
                .contains("not a readable EPUB")
        );
        let fixed = fixture(
            "<meta property='rendition:layout'>pre-paginated</meta>",
            None,
        );
        assert!(
            read_book(fixed.path())
                .unwrap_err()
                .to_string()
                .contains("Fixed-layout")
        );
        let encrypted = fixture(
            "",
            Some(
                "<encryption><EncryptedData><EncryptionMethod Algorithm='http://www.w3.org/2001/04/xmlenc#aes256'/></EncryptedData></encryption>",
            ),
        );
        assert!(
            read_book(encrypted.path())
                .unwrap_err()
                .to_string()
                .contains("Encrypted")
        );
    }

    #[test]
    fn handles_fragments_and_resource_isolation() {
        assert_eq!(
            resolve("/EPUB/text/one.xhtml", "../two.xhtml#%E7%AC%AC%E4%BA%8C"),
            Some(("/EPUB/two.xhtml".into(), Some("第二".into())))
        );
        assert_eq!(
            resolve("/EPUB/one.xhtml", "im%61ges/cover.png"),
            Some(("/EPUB/images/cover.png".into(), None))
        );
        assert_eq!(decode_fragment("%é"), "%é");
        for unsafe_href in [
            "file:///C:/secret",
            "//evil.invalid/x",
            "javascript:alert(1)",
            "data:image/png,x",
            "https://book.invalid/x",
            "..\\secret",
        ] {
            assert!(resolve("/EPUB/one.xhtml", unsafe_href).is_none());
        }
        let mut writer = LimitedBytes {
            bytes: Vec::new(),
            limit: 2,
        };
        assert!(writer.write_all(b"abc").is_err());
        assert!(writer.bytes.is_empty());
    }
}
