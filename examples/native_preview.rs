//! Offscreen native layout/GPU smoke. This never opens or controls a desktop window.
//! Synthetic books exist only in this example; chapter rendering is a static CSS
//! fixture, not a ReaderView interaction test.
use anyrender::ImageRenderer;
use anyrender_vello::VelloImageRenderer;
use blitz_dom::{BaseDocument, DocumentConfig};
use blitz_traits::shell::{ColorScheme, Viewport};
use dioxus_native::{DioxusDocument, prelude::*};
use mobi_reader::{
    controls::ActionButton,
    library_view::LibraryView,
    model::{BookRecord, ReadingLocation},
};
use std::{path::Path, sync::Arc};

#[component]
fn Preview(books: Vec<BookRecord>, chapter: bool) -> Element {
    rsx! {
        style { {include_str!("../src/theme.css")} }
        if chapter {
            div { class: "reader-shell paper",
                header { class: "reader-toolbar",
                    ActionButton { class: "button button-quiet", onpress: move |_| {}, "← Library" }
                    div { class: "reader-title", "The Art of Noticing" }
                    div { class: "reader-toolbar-actions",
                        for label in ["Contents", "Search", "Bookmarks", "Aa"] { ActionButton { class: "button button-quiet", onpress: move |_| {}, "{label}" } }
                    }
                }
                div { class: "reader-body",
                    div { class: "reader-scroll",
                        div { class: "reader-content", style: "font-size:20px;line-height:1.65;max-width:680px;font-family:'Source Serif 4',serif;",
                            div { class: "reader-block", dangerous_inner_html: "<h1>A little room for attention</h1><p>There is a particular kind of quiet that arrives when we turn the first page of a book. The room stays the same, yet its edges soften. Outside, the afternoon goes on with its ordinary business.</p><p>Reading asks for very little: a comfortable chair, a little light, and the willingness to pause. A sentence becomes a doorway. A paragraph gives us time to look around.</p><blockquote>Pay attention. Be astonished. Tell about it.</blockquote><p>In the margins of a familiar day, we can find something new. A leaf moving against a window. The sound of rain on the roof. A thought that was waiting for us to slow down.</p><p>These are not grand discoveries. They are small invitations, and a good story teaches us how to accept them.</p>" }
                        }
                    }
                }
                footer { class: "reader-footer",
                    ActionButton { class: "button button-quiet", disabled: true, onpress: move |_| {}, "← Previous" }
                    div { class: "reader-progress-copy", span { "Chapter 1 of 8" } span { "6% read" } }
                    div { class: "progress-track", div { class: "progress-fill", style: "width:6%" } }
                    ActionButton { class: "button button-quiet", onpress: move |_| {}, "Next →" }
                }
            }
        } else {
            div { class: "app-shell",
                header { class: "app-header",
                    div { class: "brand",
                        svg { width: "28", height: "28", view_box: "0 0 28 28", fill: "none", "aria-hidden": "true",
                            path { d: "M3 5C7 3 11 4 14 7C17 4 21 3 25 5V23C21 21 17 22 14 25C11 22 7 21 3 23V5Z", stroke: "#966b33", stroke_width: "1.5", stroke_linejoin: "round" }
                            path { d: "M14 7V25", stroke: "#966b33", stroke_width: "1.5" }
                        }
                        span { "mobi reader" }
                    }
                    span { class: "eyebrow", style: "margin:0", "A SPACE TO SLOW DOWN" }
                }
                LibraryView { books, on_open: move |_| {}, on_import: move |_| {}, on_settings: move |_| {}, on_remove: move |_| {} }
                footer { class: "app-footer", span { "Your books. Your quiet corner." } span { "EPUB & MOBI  /  Stored on this computer" } }
            }
        }
    }
}

fn fonts() -> dioxus_native::FontContext {
    let mut fonts = dioxus_native::FontContext::new();
    for bytes in [
        include_bytes!("../assets/fonts/SourceSerif4-Regular.otf").as_slice(),
        include_bytes!("../assets/fonts/SourceSerif4-It.otf").as_slice(),
        include_bytes!("../assets/fonts/SourceSerif4-Semibold.otf").as_slice(),
        include_bytes!("../assets/fonts/SourceSans3-Regular.otf").as_slice(),
        include_bytes!("../assets/fonts/SourceSans3-Semibold.otf").as_slice(),
    ] {
        fonts
            .collection
            .register_fonts(peniko::Blob::new(Arc::new(bytes.to_vec())), None);
    }
    fonts
}

fn fixture_books() -> Vec<BookRecord> {
    [
        ("The Art of Noticing", "Rob Walker"),
        ("A Room of One’s Own", "Virginia Woolf"),
        ("The Secret Garden", "Frances Hodgson Burnett"),
        ("Walden", "Henry David Thoreau"),
        ("The Wind in the Willows", "Kenneth Grahame"),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (title, author))| BookRecord {
        id: format!("preview-{index}"),
        title: title.into(),
        author: author.into(),
        format: "EPUB".into(),
        added_at: "2026-09-27T12:00:00Z".into(),
        last_read_at: if index == 0 {
            Some("2026-09-28T09:30:00Z".into())
        } else {
            None
        },
        cover_data: None,
        chapter_count: 8,
        location: if index == 0 {
            ReadingLocation {
                spine_index: 2,
                chapter_fraction: 0.6,
                ..ReadingLocation::default()
            }
        } else {
            ReadingLocation::default()
        },
    })
    .collect()
}

fn check_horizontal_bounds(doc: &BaseDocument, width: u32) {
    let mut pending = vec![doc.root_node().id];
    let mut buttons = 0;
    while let Some(id) = pending.pop() {
        let Some(node) = doc.get_node(id) else {
            continue;
        };
        pending.extend(node.children.iter().copied());
        if node
            .data
            .is_element_with_tag_name(&blitz_dom::LocalName::from("button"))
        {
            let rect = doc.get_client_bounding_rect(id).expect("button layout");
            if rect.width > 0.0 {
                assert!(
                    rect.x >= -1.0 && rect.x + rect.width <= width as f64 + 1.0,
                    "Button clips horizontally: {rect:?}"
                );
                buttons += 1;
            }
        }
    }
    assert!(buttons > 0, "Preview contains no visible buttons");
}

type TextSnapshot = Vec<(String, Vec<std::ops::Range<usize>>, [f64; 4])>;

fn text_snapshot(doc: &BaseDocument, scale: f32) -> TextSnapshot {
    let mut pending = vec![doc.root_node().id];
    let mut visited = std::collections::HashSet::new();
    let mut result = Vec::new();
    while let Some(id) = pending.pop() {
        if !visited.insert(id) {
            continue;
        }
        let node = doc.get_node(id).unwrap();
        pending.extend(node.children.iter().copied());
        if let Some(children) = node.layout_children.borrow().as_ref() {
            pending.extend(children.iter().copied());
        }
        if let Some(layout) = node
            .element_data()
            .and_then(|element| element.inline_layout_data.as_ref())
            && let Some(rect) = doc.get_client_bounding_rect(id)
            && !layout.text.trim().is_empty()
        {
            if ["h2", "h3"].iter().any(|tag| {
                node.data
                    .is_element_with_tag_name(&blitz_dom::LocalName::from(*tag))
            }) {
                for line in layout.layout.lines() {
                    let metrics = line.metrics();
                    assert!(
                        f64::from((metrics.advance - metrics.hanging_advance) / scale)
                            <= rect.width + 1.0,
                        "Heading text overflows its box: {:?}, width {}",
                        layout.text,
                        rect.width
                    );
                }
            }
            result.push((
                layout.text.clone(),
                layout
                    .layout
                    .lines()
                    .map(|line| line.text_range())
                    .collect(),
                [rect.x, rect.y, rect.width, rect.height],
            ));
        }
    }
    result
}

fn long_title_books() -> Vec<BookRecord> {
    let mut books = fixture_books();
    books[0].title =
        "The Master and His Emissary: The Divided Brain and the Making of the Western World".into();
    books[1].title = "A Longer Book Title That Naturally Wraps Across Several Lines".into();
    books
}

fn check_layout_stability(scale: f32) {
    let books = long_title_books();
    let mut document = DioxusDocument::new(
        VirtualDom::new_with_props(
            Preview,
            PreviewProps {
                books,
                chapter: false,
            },
        ),
        DocumentConfig {
            viewport: Some(Viewport::new(1200, 820, 1.0, ColorScheme::Light)),
            font_ctx: Some(fonts()),
            html_parser_provider: Some(Arc::new(blitz_html::HtmlProvider)),
            ..DocumentConfig::default()
        },
    );
    document.initial_build();
    let mut native = document.inner.borrow_mut();
    let mut seen = std::collections::HashMap::new();
    let widths = ((820.0 * scale) as u32..(1751.0 * scale) as u32).step_by(7);
    let steps = widths.len() * 2;
    for width in widths.clone().chain(widths.rev()) {
        native.set_viewport(Viewport::new(width, 1025, scale, ColorScheme::Light));
        native.clear_hover();
        native.resolve(0.0);
        let expected = text_snapshot(&native, scale);
        let (_, lines, tab) = expected
            .iter()
            .find(|(text, _, _)| text == "All books")
            .expect("All books tab has a text layout");
        assert_eq!(lines.len(), 1, "Shelf tab wrapped at {width}px");
        let (_, _, action) = expected
            .iter()
            .find(|(text, _, _)| text.contains("Continue reading"))
            .expect("Continue action");
        for (_, _, title) in expected
            .iter()
            .filter(|(text, _, _)| text.starts_with("The Master and His Emissary"))
        {
            assert!(
                title[0] + title[2] <= action[0] + 1.0
                    || title[1] + title[3] <= action[1] + 1.0
                    || title[1] >= action[1] + action[3] - 1.0,
                "Book title overlaps the continue action at {width}px"
            );
        }
        if let Some(previous) = seen.insert(width, expected.clone()) {
            assert_eq!(previous.len(), expected.len());
            for (old, new) in previous.iter().zip(&expected) {
                assert_eq!(
                    old, new,
                    "Resize round-trip changed text at {width}px, scale {scale}"
                );
            }
        }
        for pass in 0..6 {
            if pass % 2 == 0 {
                native.set_hover_to((tab[0] + 4.0) as f32, (tab[1] + 4.0) as f32);
            } else {
                native.clear_hover();
            }
            native.resolve(pass as f64 / 60.0);
            let actual = text_snapshot(&native, scale);
            assert_eq!(
                actual, expected,
                "Text layout changed at fixed width {width}, pass {pass}"
            );
        }
    }
    println!("Stable text line breaks through {steps} resize steps at scale {scale}");
}

fn render(
    renderer: &mut VelloImageRenderer,
    path: &str,
    width: u32,
    height: u32,
    books: Vec<BookRecord>,
    chapter: bool,
) -> anyhow::Result<()> {
    let virtual_dom = VirtualDom::new_with_props(Preview, PreviewProps { books, chapter });
    let config = DocumentConfig {
        viewport: Some(Viewport::new(width, height, 1.0, ColorScheme::Light)),
        font_ctx: Some(fonts()),
        html_parser_provider: Some(Arc::new(blitz_html::HtmlProvider)),
        ..DocumentConfig::default()
    };
    let mut document = DioxusDocument::new(virtual_dom, config);
    document.initial_build();
    let mut native = document.inner.borrow_mut();
    native.resolve(0.0);
    check_horizontal_bounds(&native, width);
    renderer.resize(width, height);
    renderer.reset();
    let mut pixels = Vec::new();
    renderer.render_to_vec(
        |painter| blitz_paint::paint_scene(painter, &mut native, 1.0, width, height, 0, 0),
        &mut pixels,
    );
    assert!(
        pixels.iter().any(|byte| *byte != 0),
        "GPU produced an empty frame"
    );
    image::save_buffer(
        Path::new(path),
        &pixels,
        width,
        height,
        image::ColorType::Rgba8,
    )?;
    println!("Saved {path} ({width}×{height})");
    Ok(())
}

fn main() -> anyhow::Result<()> {
    for scale in [1.0, 1.25, 1.5, 2.0] {
        check_layout_stability(scale);
    }
    if std::env::args().any(|argument| argument == "--layout-only") {
        return Ok(());
    }
    std::fs::create_dir_all("target/previews")?;
    let mut renderer = VelloImageRenderer::new(1200, 820);
    render(
        &mut renderer,
        "target/previews/library-long-title.png",
        1200,
        1000,
        long_title_books(),
        false,
    )?;
    render(
        &mut renderer,
        "target/previews/library-empty.png",
        1200,
        820,
        vec![],
        false,
    )?;
    render(
        &mut renderer,
        "target/previews/library-populated.png",
        1200,
        820,
        fixture_books(),
        false,
    )?;
    render(
        &mut renderer,
        "target/previews/library-compact.png",
        820,
        600,
        fixture_books(),
        false,
    )?;
    render(
        &mut renderer,
        "target/previews/chapter.png",
        1200,
        820,
        vec![],
        true,
    )?;
    Ok(())
}
