//! Generate an original synthetic book for local reader development.
//! Run with `cargo run --example make_fixture`; no downloaded book content is used.
use std::{
    fs::{self, File},
    io::Write,
    path::PathBuf,
};

use anyhow::Result;
use base64::{Engine, engine::general_purpose::STANDARD};
use zip::{ZipWriter, write::SimpleFileOptions};

const CHAPTERS: [(&str, &str); 5] = [
    (
        "The Lamp at Bay Four",
        "Mara found the reading room by following a warm square of light across the courtyard. At bay four, a brass lamp shone over an empty notebook. The room belonged to an imaginary town invented solely for this developer fixture; its visitors, shelves, and weather were made up for testing the reader.",
    ),
    (
        "The Map of Quiet Routes",
        "At the next table, Ivo unfolded a map with no roads on it. Its narrow lines joined windows, benches, and places where the rain could be heard without getting wet. Mara decided that a map of pauses might be useful even when nobody needed directions.",
    ),
    (
        "Shelves Before Dawn",
        "Before dawn, the shelves seemed taller than they had the evening before. Ivo carried a ladder slowly between the bays while Mara read the small labels aloud. They found no missing books, only a misplaced box of blank cards and a ribbon tied around yesterday's pencil.",
    ),
    (
        "The Ledger of Returns",
        "The ledger recorded the return of ordinary things: a cup, a key, a dry umbrella, and a promise to open the windows when spring arrived. Mara added a new column for things that could not be numbered. Ivo suggested patience, then waited while she made enough space for the word.",
    ),
    (
        "A Window Left Lit",
        "By evening, the notebook held a route through the imaginary room. Mara left the lamp burning at bay four so that another visitor might find the empty chair. Outside, the courtyard darkened gently, and the window became a small rectangle of welcome on the map.",
    ),
];

fn write_entry(zip: &mut ZipWriter<File>, path: &str, bytes: &[u8]) -> Result<()> {
    zip.start_file(
        path,
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
    )?;
    zip.write_all(bytes)?;
    Ok(())
}

fn main() -> Result<()> {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/fixtures");
    fs::create_dir_all(&directory)?;
    let output = directory.join("reading-room.epub");
    let mut zip = ZipWriter::new(File::create(&output)?);
    write_entry(&mut zip, "mimetype", b"application/epub+zip")?;
    write_entry(&mut zip, "META-INF/container.xml", br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#)?;
    let manifest = (1..=CHAPTERS.len()).map(|index| format!(r#"<item id="chapter{index}" href="chapter{index}.xhtml" media-type="application/xhtml+xml"/>"#)).collect::<String>();
    let spine = (1..=CHAPTERS.len())
        .map(|index| format!(r#"<itemref idref="chapter{index}"/>"#))
        .collect::<String>();
    let package = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="fixture-id">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
<dc:identifier id="fixture-id">mobi-reader-developer-fixture-reading-room-v1</dc:identifier>
<dc:title>The Reading Room — Developer Fixture</dc:title><dc:creator>Mobi Reader Test Studio</dc:creator><dc:language>en</dc:language>
<dc:description>An original, synthetic story used only to test local reader behavior.</dc:description>
<meta property="dcterms:modified">2026-09-28T00:00:00Z</meta>
</metadata><manifest>{manifest}<item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
<item id="pixel" href="images/pixel.png" media-type="image/png"/></manifest><spine>{spine}</spine></package>"#
    );
    write_entry(&mut zip, "OEBPS/content.opf", package.as_bytes())?;
    let mut navigation = String::from(
        r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><head><title>Contents</title></head><body><nav epub:type="toc" id="toc"><h1>Contents</h1><ol>"#,
    );
    for (chapter, (title, _)) in CHAPTERS.iter().enumerate() {
        let index = chapter + 1;
        navigation.push_str(&format!(r#"<li><a href="chapter{index}.xhtml">{title}</a><ol><li><a href="chapter{index}.xhtml#morning">Morning observations</a></li><li><a href="chapter{index}.xhtml#evening">Evening notes</a></li></ol></li>"#));
    }
    navigation.push_str("</ol></nav></body></html>");
    write_entry(&mut zip, "OEBPS/nav.xhtml", navigation.as_bytes())?;
    for (chapter, (title, introduction)) in CHAPTERS.iter().enumerate() {
        let index = chapter + 1;
        let mut html = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><head><title>{title}</title></head><body><h1 id="chapter-title">{title}</h1><p><strong>Synthetic developer test story.</strong> This book was generated locally for testing navigation, typography, scrolling, images, search, and saved positions.</p><p>{introduction}</p><h2 id="morning">Morning observations</h2>"#
        );
        for paragraph in 1..=48 {
            if paragraph == 25 {
                html.push_str(r#"<h2 id="evening">Evening notes</h2>"#);
            }
            html.push_str(&format!(r#"<p id="paragraph-{paragraph}">On observation {paragraph} of chapter {index}, Mara followed the quiet route from the notebook to the window. Each step gave her something different to notice: the grain of the table, the softened edge of a page, or the small reflection inside a glass of water. Ivo listened from the next bay and wrote down a question before offering an answer. They were in no hurry to finish their imaginary task. The room gave them time to look carefully, and the sentence on the page gave them a place to return. Outside, a passing cloud changed the light without moving anything indoors.</p>"#));
            if paragraph == 8 {
                html.push_str(r#"<h3 id="room-inventory">A small inventory</h3><ul><li>A brass reading lamp</li><li>A notebook with wide margins</li><li>Three cards marked <em>window</em>, <em>table</em>, and <em>door</em></li></ul><ol><li>Choose a quiet place.</li><li>Read one passage slowly.</li><li>Leave a bookmark before moving on.</li></ol>"#);
            }
            if paragraph == 16 {
                html.push_str(r#"<table><caption>Imaginary reading room ledger</caption><thead><tr><th>Bay</th><th>Object</th><th>Condition</th></tr></thead><tbody><tr><td>Four</td><td>Lamp</td><td>Lit</td></tr><tr><td>Five</td><td>Notebook</td><td>Open</td></tr><tr><td>Six</td><td>Chair</td><td>Waiting</td></tr></tbody></table><figure><img src="images/pixel.png" alt="A tiny embedded PNG used to test image handling"/><figcaption>An embedded test pixel; this is deliberately a small developer image.</figcaption></figure>"#);
            }
            if paragraph == 32 {
                html.push_str(r##"<p id="footnote-reference">Mara called this a reading-room route<a href="#note-route" epub:type="noteref">[1]</a>. A visitor could follow it in any order.</p>"##);
            }
        }
        html.push_str(r##"<aside id="note-route" epub:type="footnote"><p><strong>1.</strong> The route is fictional and belongs only to this synthetic test book. <a href="#footnote-reference">Return to the passage</a>.</p></aside><p>End of this developer fixture chapter.</p>"##);
        if index < CHAPTERS.len() {
            html.push_str(&format!(
                r#"<p><a href="chapter{}.xhtml">Continue to the next chapter</a></p>"#,
                index + 1
            ));
        }
        html.push_str("</body></html>");
        write_entry(
            &mut zip,
            &format!("OEBPS/chapter{index}.xhtml"),
            html.as_bytes(),
        )?;
    }
    let pixel = STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=")?;
    write_entry(&mut zip, "OEBPS/images/pixel.png", &pixel)?;
    zip.finish()?;
    println!("Generated synthetic EPUB fixture: {}", output.display());
    Ok(())
}
