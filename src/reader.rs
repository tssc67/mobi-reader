use crate::controls::ActionButton;
use dioxus_native::prelude::*;
use std::{cell::RefCell, rc::Rc, sync::Arc, time::Duration};

use crate::{
    ebook::search_book,
    model::{BookDocument, BookRecord, Bookmark, ReaderPreferences, ReadingLocation, SearchHit},
    viewport::Viewport,
};

fn block_order(document: &BookDocument, index: usize) -> Vec<String> {
    document
        .chapters
        .get(index)
        .map(|chapter| {
            chapter
                .blocks
                .iter()
                .map(|block| block.id.clone())
                .collect()
        })
        .unwrap_or_default()
}

fn jump(
    document: &BookDocument,
    mut chapter: Signal<usize>,
    viewport: &Viewport,
    mut location: ReadingLocation,
    on_location: EventHandler<ReadingLocation>,
    on_error: EventHandler<String>,
) {
    if document.chapters.is_empty() {
        return;
    }
    location.spine_index = location.spine_index.min(document.chapters.len() - 1);
    viewport.prepare(
        location.clone(),
        block_order(document, location.spine_index),
    );
    chapter.set(location.spine_index);
    viewport.restore(on_location, on_error);
}

fn fragment_location(
    document: &BookDocument,
    index: usize,
    fragment: Option<&str>,
) -> ReadingLocation {
    let block_id = document
        .chapters
        .get(index)
        .and_then(|chapter| {
            fragment
                .and_then(|fragment| {
                    chapter.blocks.iter().find(|block| {
                        block.id == fragment
                            || block.source_ids.iter().any(|source| source == fragment)
                    })
                })
                .or_else(|| chapter.blocks.first())
        })
        .map(|block| block.id.clone())
        .unwrap_or_default();
    ReadingLocation {
        spine_index: index,
        block_id,
        ..ReadingLocation::default()
    }
}

fn decode_fragment(fragment: &str) -> String {
    // URL fragments use percent encoding, not form encoding (a plus remains a plus).
    let mut bytes = Vec::new();
    let input = fragment.as_bytes();
    let mut index = 0;
    while index < input.len() {
        if input[index] == b'%' && index + 2 < input.len() {
            let hex = |byte| match byte {
                b'0'..=b'9' => Some(byte - b'0'),
                b'a'..=b'f' => Some(byte - b'a' + 10),
                b'A'..=b'F' => Some(byte - b'A' + 10),
                _ => None,
            };
            if let (Some(high), Some(low)) = (hex(input[index + 1]), hex(input[index + 2])) {
                bytes.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        bytes.push(input[index]);
        index += 1;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn run_search(
    document: Arc<BookDocument>,
    query: Signal<String>,
    mut hits: Signal<Vec<SearchHit>>,
    mut searching: Signal<bool>,
    mut search_ran: Signal<bool>,
    mut search_version: Signal<u64>,
    on_error: EventHandler<String>,
) {
    let text = query();
    let version = search_version().wrapping_add(1);
    search_version.set(version);
    hits.set(Vec::new());
    search_ran.set(!text.trim().is_empty());
    if text.trim().is_empty() {
        searching.set(false);
        return;
    }
    searching.set(true);
    spawn(async move {
        let results = tokio::task::spawn_blocking(move || search_book(&document, &text)).await;
        if search_version() != version {
            return;
        }
        searching.set(false);
        match results {
            Ok(results) => hits.set(results),
            Err(error) => on_error.call(format!("Book search failed: {error}")),
        }
    });
}

#[component]
pub fn ReaderView(
    book: BookRecord,
    document: Arc<BookDocument>,
    preferences: ReaderPreferences,
    bookmarks: Vec<Bookmark>,
    on_back: EventHandler<()>,
    on_location: EventHandler<ReadingLocation>,
    on_preferences: EventHandler<ReaderPreferences>,
    on_add_bookmark: EventHandler<(String, ReadingLocation)>,
    on_remove_bookmark: EventHandler<i64>,
    on_error: EventHandler<String>,
) -> Element {
    let initial_index = book
        .location
        .spine_index
        .min(document.chapters.len().saturating_sub(1));
    let chapter = use_signal(|| initial_index);
    let mut panel = use_signal(String::new);
    let mut query = use_signal(String::new);
    let mut hits = use_signal(Vec::<SearchHit>::new);
    let mut searching = use_signal(|| false);
    let mut search_ran = use_signal(|| false);
    let mut search_version = use_signal(|| 0u64);
    let mut shown_location = use_signal(|| book.location.clone());
    let viewport = use_hook(|| {
        let mut location = book.location.clone();
        location.spine_index = initial_index;
        Viewport::new(location, block_order(&document, initial_index))
    });
    let previous_preferences = use_hook(|| Rc::new(RefCell::new(preferences.clone())));
    if *previous_preferences.borrow() != preferences {
        // Settings can also change appearance outside this sidebar (for example
        // through the import result panel). Keep the same anchor for those updates.
        let location = viewport.capture();
        viewport.prepare(
            location.clone(),
            block_order(&document, location.spine_index),
        );
        viewport.restore(on_location, on_error);
        *previous_preferences.borrow_mut() = preferences.clone();
    }
    let monitor_viewport = viewport.clone();
    use_future(move || {
        let viewport = monitor_viewport.clone();
        async move {
            let mut last = viewport.location();
            loop {
                tokio::time::sleep(Duration::from_millis(250)).await;
                let current = viewport.capture();
                if current != last {
                    shown_location.set(current.clone());
                    on_location.call(current.clone());
                    last = current;
                }
            }
        }
    });
    let drop_viewport = viewport.clone();
    use_drop(move || on_location.call(drop_viewport.capture()));
    let window_viewport = viewport.clone();
    let window_document = document.clone();
    dioxus_native::use_window_event(move |event, _| {
        use dioxus_native::winit::event::WindowEvent;
        match event {
            WindowEvent::CloseRequested => on_location.call(window_viewport.capture()),
            WindowEvent::SurfaceResized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                let location = window_viewport.location();
                window_viewport.prepare(
                    location.clone(),
                    block_order(&window_document, location.spine_index),
                );
                window_viewport.restore(on_location, on_error);
            }
            _ => {}
        }
    });

    let Some(current) = document.chapters.get(chapter()).cloned() else {
        return rsx! { div { class: "empty-reader", h2 { "This book has no readable chapters." } ActionButton { class: "button", onpress: move |_| on_back.call(()), "Back to library" } } };
    };
    let root_class = format!("reader-shell {}", preferences.theme);
    let family = if preferences.serif {
        "'Source Serif 4', serif"
    } else {
        "'Source Sans 3', sans-serif"
    };
    let content_style = format!(
        "font-size: {}px; line-height: {}; max-width: {}px; font-family: {};",
        preferences.font_size, preferences.line_height, preferences.column_width, family
    );
    let whole_progress = ((chapter() as f64 + shown_location().chapter_fraction)
        / document.chapters.len() as f64)
        .clamp(0.0, 1.0);
    let progress_style = format!("width: {}%;", whole_progress * 100.0);
    let chapter_label = format!("Chapter {} of {}", chapter() + 1, document.chapters.len());
    let progress_label = format!("{:.0}% read", whole_progress * 100.0);
    let back_viewport = viewport.clone();
    let mounted_viewport = viewport.clone();
    let link_viewport = viewport.clone();
    let link_document = document.clone();
    let previous_viewport = viewport.clone();
    let previous_document = document.clone();
    let next_viewport = viewport.clone();
    let next_document = document.clone();
    let search_document = document.clone();
    let search_keyboard_document = document.clone();
    let key_viewport = viewport.clone();
    let key_document = document.clone();
    let scroll_viewport = viewport.clone();
    let wheel_viewport = viewport.clone();
    let scroll_key_viewport = viewport.clone();

    rsx! {
        div { class: "{root_class}",
            onkeydown: move |event| {
                if event.key() == Key::Escape && !panel().is_empty() {
                    let location = key_viewport.capture();
                    key_viewport.prepare(location.clone(), block_order(&key_document, location.spine_index));
                    panel.set(String::new());
                    key_viewport.restore(on_location, on_error);
                }
                if event.modifiers().contains(Modifiers::CONTROL) && matches!(event.key(), Key::Character(ref value) if value.eq_ignore_ascii_case("f")) {
                    event.prevent_default();
                    let location = key_viewport.capture();
                    key_viewport.prepare(location.clone(), block_order(&key_document, location.spine_index));
                    panel.set("search".into());
                    key_viewport.restore(on_location, on_error);
                }
            },
            header { class: "reader-toolbar",
                ActionButton { class: "button button-quiet", onpress: move |_| { on_location.call(back_viewport.capture()); on_back.call(()); }, "← Library" }
                div { class: "reader-title", "{book.title}" }
                div { class: "reader-toolbar-actions",
                    for (value, label) in [("contents", "Contents"), ("search", "Search"), ("bookmarks", "Bookmarks"), ("type", "Aa")] {
                        {
                            let toggle_viewport = viewport.clone();
                            let toggle_document = document.clone();
                            rsx! { ActionButton { class: if panel() == value { "button button-quiet toolbar-active" } else { "button button-quiet" }, aria_label: if value == "type" { "Reading appearance" } else { label }, aria_pressed: panel() == value,
                                onpress: move |_| {
                                    let location = toggle_viewport.capture();
                                    toggle_viewport.prepare(location.clone(), block_order(&toggle_document, location.spine_index));
                                    if panel() == value { panel.set(String::new()); } else { panel.set(value.to_string()); }
                                    toggle_viewport.restore(on_location, on_error);
                                }, "{label}"
                            } }
                        }
                    }
                }
            }
            div { class: "reader-body",
                if !panel().is_empty() {
                    aside { class: "reader-sidebar",
                        div { class: "reader-panel-heading",
                            h2 { {match panel().as_str() { "contents" => "Contents", "search" => "Find in book", "bookmarks" => "Bookmarks", _ => "Reading appearance" }} }
                            {
                                let close_viewport = viewport.clone();
                                let close_document = document.clone();
                                rsx! { ActionButton { class: "icon-button", aria_label: "Close sidebar", onpress: move |_| {
                                    let location = close_viewport.capture();
                                    close_viewport.prepare(location.clone(), block_order(&close_document, location.spine_index));
                                    panel.set(String::new()); close_viewport.restore(on_location, on_error);
                                }, "×" } }
                            }
                        }
                        if panel() == "contents" {
                            nav { aria_label: "Table of contents",
                                for entry in document.toc.iter().cloned() {
                                    {
                                        let target = fragment_location(&document, entry.spine_index, entry.fragment.as_deref());
                                        let target_viewport = viewport.clone();
                                        let target_document = document.clone();
                                        let padding = format!("padding-left: {}px;", 12 + entry.depth.min(6) * 12);
                                        rsx! { ActionButton { class: if entry.spine_index == chapter() { "toc-item active" } else { "toc-item" }, style: "{padding}", onpress: move |_| jump(&target_document, chapter, &target_viewport, target.clone(), on_location, on_error), "{entry.title}" } }
                                    }
                                }
                            }
                        } else if panel() == "search" {
                            div { class: "reader-search",
                                input { class: "field", r#type: "search", placeholder: "A word or phrase…", aria_label: "Search the whole book", value: "{query}",
                                    oninput: move |event| { query.set(event.value()); search_version += 1; searching.set(false); search_ran.set(false); hits.set(Vec::new()); },
                                    onkeydown: move |event| { if event.key() == Key::Enter { event.prevent_default(); run_search(search_keyboard_document.clone(), query, hits, searching, search_ran, search_version, on_error); } },
                                    onmounted: move |event| { let mounted = event.data(); spawn(async move { tokio::time::sleep(Duration::from_millis(10)).await; let _ = mounted.set_focus(true).await; }); }
                                }
                                ActionButton { class: "button button-primary", onpress: move |_| run_search(search_document.clone(), query, hits, searching, search_ran, search_version, on_error), "Find" }
                            }
                            if searching() { p { class: "muted sidebar-hint", "Searching your book…" } }
                            else if hits().is_empty() { p { class: "muted sidebar-hint", if search_ran() { "No matches. Try a different word or phrase." } else { "Enter a word or phrase, then choose Find." } } }
                            else {
                                p { class: "muted sidebar-hint", "{hits().len()} matches" }
                                for hit in hits() {
                                    {
                                        let target = ReadingLocation { spine_index: hit.spine_index, block_id: hit.block_id.clone(), ..ReadingLocation::default() };
                                        let target_viewport = viewport.clone();
                                        let target_document = document.clone();
                                        rsx! { ActionButton { class: "search-hit", onpress: move |_| jump(&target_document, chapter, &target_viewport, target.clone(), on_location, on_error),
                                            strong { "{hit.chapter_title}" } p { "{hit.snippet}" }
                                        } }
                                    }
                                }
                            }
                        } else if panel() == "bookmarks" {
                            {
                                let bookmark_viewport = viewport.clone();
                                let title = current.title.clone();
                                rsx! { ActionButton { class: "button button-primary bookmark-add", onpress: move |_| on_add_bookmark.call((title.clone(), bookmark_viewport.capture())), "+ Bookmark this place" } }
                            }
                            if bookmarks.is_empty() { p { class: "muted sidebar-hint", "Save a place you would like to return to." } }
                            for bookmark in bookmarks.iter().cloned() {
                                {
                                    let target_viewport = viewport.clone();
                                    let target_document = document.clone();
                                    let target = bookmark.location.clone();
                                    let bookmark_id = bookmark.id;
                                    rsx! { div { class: "bookmark-row",
                                        ActionButton { class: "bookmark-item", onpress: move |_| jump(&target_document, chapter, &target_viewport, target.clone(), on_location, on_error), "{bookmark.label}" }
                                        ActionButton { class: "icon-button", aria_label: format!("Remove bookmark {}", bookmark.label), onpress: move |_| on_remove_bookmark.call(bookmark_id), "×" }
                                    } }
                                }
                            }
                        } else {
                            ReaderAppearance { preferences: preferences.clone(), on_change: {
                                let type_viewport = viewport.clone();
                                let type_document = document.clone();
                                move |prefs| {
                                    let location = type_viewport.capture();
                                    type_viewport.prepare(location.clone(), block_order(&type_document, location.spine_index));
                                    on_preferences.call(prefs);
                                    type_viewport.restore(on_location, on_error);
                                }
                            } }
                        }
                    }
                }
                div { class: "reader-scroll", tabindex: "0", aria_label: "Book chapter",
                    onscroll: move |_| { scroll_viewport.capture(); },
                    onwheel: move |_| wheel_viewport.user_scroll(),
                    onkeydown: move |event| {
                        if scroll_key_viewport.keyboard_scroll(&event.key(), event.modifiers().contains(Modifiers::SHIFT)) {
                            event.prevent_default();
                        }
                    },
                    onmounted: move |event| {
                        let mounted = event.data();
                        mounted_viewport.mount_scroll(mounted.clone()); mounted_viewport.restore(on_location, on_error);
                        spawn(async move { tokio::time::sleep(Duration::from_millis(10)).await; let _ = mounted.set_focus(true).await; });
                    },
                    div { class: "reader-content", style: "{content_style}",
                        onclick: move |event| {
                            event.prevent_default();
                            let point = event.client_coordinates();
                            if let Some(href) = link_viewport.link_at(point.x, point.y)
                                && let Ok(url) = url::Url::parse(&href)
                                && url.scheme() == "reader" && url.host_str() == Some("chapter")
                                && let Ok(index) = url.path().trim_matches('/').parse::<usize>()
                                && index < link_document.chapters.len() {
                                let fragment = url.fragment().map(decode_fragment);
                                let target = fragment_location(&link_document, index, fragment.as_deref());
                                jump(&link_document, chapter, &link_viewport, target, on_location, on_error);
                            }
                        },
                        if current.blocks.is_empty() { p { class: "muted", "This chapter contains no readable text." } }
                        for block in current.blocks {
                            {
                                let block_viewport = viewport.clone();
                                let id = block.id.clone();
                                rsx! { div { class: "reader-block", id: "{block.id}", key: "{block.id}", onmounted: move |event| block_viewport.mount_block(id.clone(), event.data()), dangerous_inner_html: "{block.html}" } }
                            }
                        }
                    }
                }
            }
            footer { class: "reader-footer",
                ActionButton { class: "button button-quiet", disabled: chapter() == 0,
                    onpress: move |_| {
                        let index = chapter().saturating_sub(1);
                        let target = fragment_location(&previous_document, index, None);
                        jump(&previous_document, chapter, &previous_viewport, target, on_location, on_error);
                    }, "← Previous"
                }
                div { class: "reader-progress-copy", span { "{chapter_label}" } span { "{progress_label}" } }
                div { class: "progress-track", div { class: "progress-fill", style: "{progress_style}" } }
                ActionButton { class: "button button-quiet", disabled: chapter() + 1 >= document.chapters.len(),
                    onpress: move |_| {
                        let index = (chapter() + 1).min(next_document.chapters.len() - 1);
                        let target = fragment_location(&next_document, index, None);
                        jump(&next_document, chapter, &next_viewport, target, on_location, on_error);
                    }, "Next →"
                }
            }
        }
    }
}

#[component]
fn ReaderAppearance(
    preferences: ReaderPreferences,
    on_change: EventHandler<ReaderPreferences>,
) -> Element {
    let size_down = preferences.clone();
    let size_up = preferences.clone();
    let height_down = preferences.clone();
    let height_up = preferences.clone();
    let width_down = preferences.clone();
    let width_up = preferences.clone();
    rsx! {
        div { class: "reading-appearance",
            div { class: "appearance-field", span { class: "field-label", "Text size" }
                div { class: "appearance-stepper",
                    ActionButton { class: "icon-button", aria_label: "Decrease text size", disabled: preferences.font_size <= 14, onpress: move |_| { let mut prefs = size_down.clone(); prefs.font_size = prefs.font_size.saturating_sub(1).clamp(14, 36); on_change.call(prefs); }, "−" }
                    span { "{preferences.font_size}px" }
                    ActionButton { class: "icon-button", aria_label: "Increase text size", disabled: preferences.font_size >= 36, onpress: move |_| { let mut prefs = size_up.clone(); prefs.font_size = prefs.font_size.saturating_add(1).clamp(14, 36); on_change.call(prefs); }, "+" }
                }
            }
            div { class: "appearance-field", span { class: "field-label", "Line spacing" }
                div { class: "appearance-stepper",
                    ActionButton { class: "icon-button", aria_label: "Decrease line spacing", disabled: preferences.line_height <= 1.2, onpress: move |_| { let mut prefs = height_down.clone(); prefs.line_height = (prefs.line_height - 0.05).clamp(1.2, 2.2); on_change.call(prefs); }, "−" }
                    span { "{preferences.line_height:.2}" }
                    ActionButton { class: "icon-button", aria_label: "Increase line spacing", disabled: preferences.line_height >= 2.2, onpress: move |_| { let mut prefs = height_up.clone(); prefs.line_height = (prefs.line_height + 0.05).clamp(1.2, 2.2); on_change.call(prefs); }, "+" }
                }
            }
            div { class: "appearance-field", span { class: "field-label", "Column width" }
                div { class: "appearance-stepper",
                    ActionButton { class: "icon-button", aria_label: "Narrower column", disabled: preferences.column_width <= 440, onpress: move |_| { let mut prefs = width_down.clone(); prefs.column_width = prefs.column_width.saturating_sub(20).clamp(440, 960); on_change.call(prefs); }, "−" }
                    span { "{preferences.column_width}px" }
                    ActionButton { class: "icon-button", aria_label: "Wider column", disabled: preferences.column_width >= 960, onpress: move |_| { let mut prefs = width_up.clone(); prefs.column_width = prefs.column_width.saturating_add(20).clamp(440, 960); on_change.call(prefs); }, "+" }
                }
            }
            div { class: "appearance-field", span { class: "field-label", "Typeface" }
                div { class: "font-options",
                    for (serif, label) in [(true, "Source Serif 4"), (false, "Source Sans 3")] {
                        {
                            let prefs = preferences.clone();
                            rsx! { ActionButton { class: if preferences.serif == serif { "shelf-tab active" } else { "shelf-tab" }, aria_pressed: preferences.serif == serif, onpress: move |_| { let mut prefs = prefs.clone(); prefs.serif = serif; on_change.call(prefs); }, "{label}" } }
                        }
                    }
                }
            }
            span { class: "field-label", "Page tone" }
            div { class: "theme-options",
                for (value, label) in [("paper", "Paper"), ("sepia", "Sepia"), ("night", "Night")] {
                    {
                        let prefs = preferences.clone();
                        let class = format!("theme-option {} {}", value, if preferences.theme == value { "active" } else { "" });
                        rsx! { ActionButton { class: "{class}", aria_pressed: preferences.theme == value, onpress: move |_| { let mut prefs = prefs.clone(); prefs.theme = value.into(); on_change.call(prefs); }, "{label}" } }
                    }
                }
            }
        }
    }
}
