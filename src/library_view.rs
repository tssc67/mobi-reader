use crate::controls::ActionButton;
use dioxus_native::prelude::*;

use crate::model::BookRecord;

#[component]
pub fn LibraryView(
    books: Vec<BookRecord>,
    on_open: EventHandler<String>,
    on_import: EventHandler<()>,
    on_settings: EventHandler<()>,
    on_remove: EventHandler<String>,
) -> Element {
    let mut query = use_signal(String::new);
    let mut shelf = use_signal(|| "all".to_string());
    let mut sort = use_signal(|| "recent".to_string());
    let mut removal = use_signal(|| None::<BookRecord>);
    let search = query().trim().to_lowercase();
    let mut visible: Vec<BookRecord> = books
        .iter()
        .filter(|book| {
            let matches_query = search.is_empty()
                || book.title.to_lowercase().contains(&search)
                || book.author.to_lowercase().contains(&search);
            let matches_shelf = match shelf().as_str() {
                "reading" => book.last_read_at.is_some() && book.progress() < 0.99,
                "finished" => book.progress() >= 0.99,
                _ => true,
            };
            matches_query && matches_shelf
        })
        .cloned()
        .collect();
    if sort() == "title" {
        visible.sort_by_key(|book| book.title.to_lowercase());
    } else {
        visible.sort_by(|left, right| {
            right
                .last_read_at
                .as_ref()
                .unwrap_or(&right.added_at)
                .cmp(left.last_read_at.as_ref().unwrap_or(&left.added_at))
        });
    }
    let current = books
        .iter()
        .filter(|book| book.last_read_at.is_some() && book.progress() < 0.99)
        .max_by(|left, right| left.last_read_at.cmp(&right.last_read_at))
        .cloned();
    let count_label = if books.len() == 1 {
        "1 book".to_string()
    } else {
        format!("{} books", books.len())
    };

    rsx! {
        div { class: "library-view",
            header { class: "library-heading",
                div {
                    p { class: "eyebrow", "YOUR OWN LITTLE WORLD" }
                    h1 { "The library" }
                    p { class: "muted", "A quiet place for your next chapter." }
                }
                div { class: "library-heading-actions",
                    ActionButton { class: "button button-quiet", onpress: move |_| on_settings.call(()), "Settings" }
                    ActionButton { class: "button button-primary", onpress: move |_| on_import.call(()), "+ Import books" }
                }
            }

            if books.is_empty() {
                section { class: "library-empty",
                    div { class: "empty-book-mark", aria_hidden: "true", "aA" }
                    p { class: "eyebrow", "MAKE ROOM FOR A GOOD STORY" }
                    h2 { "Your first chapter starts here." }
                    p { class: "empty-description", "Bring your books together in one personal library. Import an ebook, settle in, and pick up exactly where you left off." }
                    ActionButton { class: "button button-primary", onpress: move |_| on_import.call(()), "Import your first book" }
                    p { class: "muted import-hint", "Or drop your ebook files anywhere in this window." }
                }
            } else {
                if let Some(book) = current {
                    {
                        let id = book.id.clone();
                        let progress = format!("{:.0}%", book.progress() * 100.0);
                        let progress_style = format!("width: {}%;", book.progress() * 100.0);
                        rsx! {
                            section { class: "continue-reading",
                                div { class: "continue-cover", BookCover { book: book.clone() } }
                                div { class: "continue-copy",
                                    p { class: "eyebrow", "PICK UP WHERE YOU LEFT OFF" }
                                    h2 { "{book.title}" }
                                    p { class: "muted", "{book.author}" }
                                    div { class: "continue-progress",
                                        div { class: "progress-track", div { class: "progress-fill", style: "{progress_style}" } }
                                        span { class: "muted", "{progress} read" }
                                    }
                                }
                                ActionButton { class: "button button-primary continue-action", onpress: move |_| on_open.call(id.clone()), "Continue reading →" }
                            }
                        }
                    }
                }

                section { class: "library-shelf",
                    div { class: "shelf-heading",
                        h2 { "Your collection" }
                        span { class: "muted", "{count_label}" }
                    }
                    div { class: "library-controls",
                        div { class: "shelf-tabs",
                            for (value, label) in [("all", "All books"), ("reading", "Reading"), ("finished", "Finished")] {
                                ActionButton {
                                    class: if shelf() == value { "shelf-tab active" } else { "shelf-tab" },
                                    aria_pressed: shelf() == value,
                                    onpress: move |_| shelf.set(value.to_string()),
                                    "{label}"
                                }
                            }
                        }
                        div { class: "library-filter-controls",
                            label { class: "library-search-group", span { "Search" }
                                input { class: "field library-search", r#type: "search", aria_label: "Search books by title or author", placeholder: "Search title or author…", value: "{query}", oninput: move |event| query.set(event.value()) }
                            }
                            div { class: "library-sort sort-switch", aria_label: "Sort books",
                                ActionButton { class: if sort() == "recent" { "shelf-tab active" } else { "shelf-tab" }, aria_pressed: sort() == "recent", onpress: move |_| sort.set("recent".into()), "Recent" }
                                ActionButton { class: if sort() == "title" { "shelf-tab active" } else { "shelf-tab" }, aria_pressed: sort() == "title", onpress: move |_| sort.set("title".into()), "A–Z" }
                            }
                        }
                    }
                    if visible.is_empty() {
                        div { class: "shelf-empty",
                            h3 { "No books here yet." }
                            p { class: "muted", "Try another shelf or search for a different title or author." }
                            ActionButton { class: "button button-quiet", onpress: move |_| { query.set(String::new()); shelf.set("all".into()); }, "Show all books" }
                        }
                    } else {
                        div { class: "book-grid",
                            for book in visible {
                                {
                                    let id = book.id.clone();
                                    let remove_book = book.clone();
                                    let progress = book.progress();
                                    let status = if progress >= 0.99 { "Finished".to_string() }
                                        else if book.last_read_at.is_some() { format!("{:.0}% read", progress * 100.0) }
                                        else { "Unread".to_string() };
                                    rsx! {
                                        article { class: "book-card", key: "{book.id}",
                                            ActionButton { class: "book-open", aria_label: format!("Read {}", book.title), onpress: move |_| on_open.call(id.clone()),
                                                BookCover { book: book.clone() }
                                                h3 { "{book.title}" }
                                                p { class: "book-author", "{book.author}" }
                                            }
                                            div { class: "book-meta",
                                                span { class: "muted", "{status}" }
                                                ActionButton { class: "book-remove", aria_label: format!("Remove {} from library", book.title), onpress: move |_| removal.set(Some(remove_book.clone())), "Remove" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if let Some(book) = removal() {
                div { class: "modal-backdrop",
                    section { class: "modal", role: "dialog", aria_modal: "true", aria_label: "Remove book from library",
                        p { class: "eyebrow", "YOUR LIBRARY" }
                        h2 { "Remove this book?" }
                        p { "“{book.title}” will be removed from your library. The original ebook file will stay on your computer." }
                        div { class: "modal-actions",
                            ActionButton { class: "button button-quiet", onpress: move |_| removal.set(None), "Keep book" }
                            ActionButton { class: "button button-danger", onpress: move |_| { on_remove.call(book.id.clone()); removal.set(None); }, "Remove book" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn BookCover(book: BookRecord) -> Element {
    let cover_class = match book
        .id
        .bytes()
        .fold(0usize, |total, byte| total.wrapping_add(byte as usize))
        % 4
    {
        0 => "book-cover cover-sage",
        1 => "book-cover cover-amber",
        2 => "book-cover cover-blue",
        _ => "book-cover cover-rose",
    };
    rsx! {
        if let Some(source) = book.cover_data {
            img { class: "book-cover cover-image", src: "{source}", alt: format!("Cover of {}", book.title) }
        } else {
            div { class: "{cover_class}", aria_hidden: "true",
                span { class: "cover-format", "{book.format}" }
                div { class: "cover-letterpress", "{book.title}" }
                span { class: "cover-rule" }
                span { class: "cover-author", "{book.author}" }
            }
        }
    }
}
