use crate::{
    controls::ActionButton, ebook::read_book, library::Library, library_view::LibraryView,
    model::*, persistence::Persistence, reader::ReaderView, settings::Settings,
};
use dioxus_native::prelude::*;
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Clone)]
pub struct Services {
    pub library: Option<Library>,
    pub persistence: Option<Persistence>,
    pub cancel: Arc<AtomicBool>,
    books: Vec<BookRecord>,
    preferences: ReaderPreferences,
    error: Option<String>,
}

impl Services {
    pub fn load() -> Self {
        let mut service = Self {
            library: None,
            persistence: None,
            cancel: Arc::new(AtomicBool::new(false)),
            books: vec![],
            preferences: ReaderPreferences::default(),
            error: None,
        };
        match Library::open() {
            Ok(library) => {
                match library.list_books() {
                    Ok(books) => service.books = books,
                    Err(error) => {
                        service.error = Some(format!("Cannot load your library: {error:#}"))
                    }
                }
                match library.load_preferences() {
                    Ok(prefs) => service.preferences = normalize_preferences(prefs),
                    Err(error) => {
                        service.error = Some(format!("Cannot load preferences: {error:#}"))
                    }
                }
                service.persistence = Some(Persistence::new(library.clone()));
                service.library = Some(library);
            }
            Err(error) => service.error = Some(format!("Cannot open your library: {error:#}")),
        }
        service
    }
}

pub fn normalize_preferences(mut prefs: ReaderPreferences) -> ReaderPreferences {
    prefs.font_size = prefs.font_size.clamp(14, 36);
    prefs.column_width = prefs.column_width.clamp(440, 960);
    prefs.line_height = if prefs.line_height.is_finite() {
        prefs.line_height.clamp(1.2, 2.2)
    } else {
        1.65
    };
    if !matches!(prefs.theme.as_str(), "paper" | "sepia" | "night") {
        prefs.theme = "paper".into();
    }
    prefs
}

#[derive(Clone)]
struct Session {
    book: BookRecord,
    document: Arc<BookDocument>,
}

#[derive(Clone, PartialEq)]
struct Job {
    path: PathBuf,
    state: ImportJob,
}

#[derive(Clone, Copy)]
struct State {
    services: Signal<Services>,
    books: Signal<Vec<BookRecord>>,
    preferences: Signal<ReaderPreferences>,
    session: Signal<Option<Session>>,
    bookmarks: Signal<Vec<Bookmark>>,
    settings: Signal<bool>,
    message: Signal<Option<String>>,
    jobs: Signal<Vec<Job>>,
    pending: Signal<VecDeque<usize>>,
    importing: Signal<bool>,
    opening: Signal<bool>,
    open_request: Signal<u64>,
    latest_location: Signal<Option<(String, ReadingLocation)>>,
}

impl State {
    fn error(mut self, message: impl Into<String>) {
        self.message.set(Some(message.into()));
    }

    fn choose_books(self) {
        spawn(async move {
            let result = tokio::task::spawn_blocking(|| {
                rfd::FileDialog::new()
                    .set_title("Add books to your library")
                    .add_filter("Ebooks", &["epub", "mobi"])
                    .pick_files()
            })
            .await;
            match result {
                Ok(Some(paths)) => self.enqueue(paths),
                Ok(None) => {}
                Err(error) => self.error(format!("Cannot open the file picker: {error}")),
            }
        });
    }

    fn enqueue(mut self, paths: Vec<PathBuf>) {
        if self.services.peek().library.is_none() {
            self.error(
                "Your library is unavailable. Check its folder permissions and restart the app.",
            );
            return;
        }
        for path in paths {
            let filename = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let index = self.jobs.peek().len();
            self.jobs.write().push(Job {
                path,
                state: ImportJob {
                    filename,
                    status: "Queued".into(),
                    error: None,
                },
            });
            self.pending.write().push_back(index);
        }
        if *self.importing.peek() || self.pending.peek().is_empty() {
            return;
        }
        self.importing.set(true);
        self.services.peek().cancel.store(false, Ordering::Relaxed);
        spawn(async move {
            loop {
                let next = self.pending.write().pop_front();
                let Some(index) = next else {
                    break;
                };
                let cancel = self.services.peek().cancel.clone();
                if cancel.load(Ordering::Relaxed) {
                    self.jobs.write()[index].state.status = "Cancelled".into();
                    continue;
                }
                let Some(library) = self.services.peek().library.clone() else {
                    break;
                };
                let path = self.jobs.peek()[index].path.clone();
                self.jobs.write()[index].state.status = if path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("mobi"))
                {
                    "Converting and importing...".into()
                } else {
                    "Importing...".into()
                };
                let result =
                    tokio::task::spawn_blocking(move || library.import_book(&path, &cancel)).await;
                match result {
                    Ok(Ok(record)) => {
                        let exists = self.books.peek().iter().any(|book| book.id == record.id);
                        self.jobs.write()[index].state.status = if exists {
                            "Already in your library".into()
                        } else {
                            "Added to your library".into()
                        };
                        if !exists {
                            self.books.write().insert(0, record);
                        }
                    }
                    Ok(Err(error)) => {
                        let mut jobs = self.jobs.write();
                        jobs[index].state.status =
                            if self.services.peek().cancel.load(Ordering::Relaxed) {
                                "Cancelled".into()
                            } else {
                                "Could not import".into()
                            };
                        jobs[index].state.error = Some(format!("{error:#}"));
                    }
                    Err(error) => {
                        let mut jobs = self.jobs.write();
                        jobs[index].state.status = "Import failed".into();
                        jobs[index].state.error = Some(error.to_string());
                    }
                }
            }
            self.importing.set(false);
        });
    }

    fn open_book(mut self, id: String) {
        let Some(book) = self.books.peek().iter().find(|book| book.id == id).cloned() else {
            return;
        };
        let Some(library) = self.services.peek().library.clone() else {
            return;
        };
        let path = library.book_path(&id);
        let request = *self.open_request.peek() + 1;
        self.open_request.set(request);
        self.opening.set(true);
        spawn(async move {
            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
                Ok((read_book(&path)?, library.bookmarks(&id)?))
            })
            .await;
            if *self.open_request.peek() != request {
                return;
            }
            self.opening.set(false);
            match result {
                Ok(Ok((document, bookmarks))) => {
                    let mut book = book;
                    book.location.spine_index = book
                        .location
                        .spine_index
                        .min(document.chapters.len().saturating_sub(1));
                    self.bookmarks.set(bookmarks);
                    self.session.set(Some(Session {
                        book: book.clone(),
                        document: Arc::new(document),
                    }));
                    self.location(book.location);
                }
                Ok(Err(error)) => self.error(format!("Cannot open this book: {error:#}")),
                Err(error) => self.error(format!("Cannot open this book: {error}")),
            }
        });
    }

    fn location(mut self, location: ReadingLocation) {
        let Some(id) = self
            .session
            .peek()
            .as_ref()
            .map(|session| session.book.id.clone())
        else {
            return;
        };
        self.latest_location
            .set(Some((id.clone(), location.clone())));
        let persistence = self.services.peek().persistence.clone();
        if let Some(persistence) = persistence
            && let Err(error) = persistence.save_location(id, location)
        {
            self.error(error.to_string());
        }
    }

    fn back_to_library(mut self) {
        let latest = self.latest_location.peek().clone();
        if let Some((id, location)) = latest
            && let Some(book) = self.books.write().iter_mut().find(|book| book.id == id)
        {
            book.location = location;
            book.last_read_at = Some(chrono::Utc::now().to_rfc3339());
        }
        self.session.set(None);
    }

    fn preferences(mut self, preferences: ReaderPreferences) {
        let preferences = normalize_preferences(preferences);
        self.preferences.set(preferences.clone());
        let persistence = self.services.peek().persistence.clone();
        if let Some(persistence) = persistence
            && let Err(error) = persistence.save_preferences(preferences)
        {
            self.error(error.to_string());
        }
    }

    fn remove_book(mut self, id: String) {
        let Some(library) = self.services.peek().library.clone() else {
            return;
        };
        spawn(async move {
            let removed = id.clone();
            match tokio::task::spawn_blocking(move || library.remove_book(&removed)).await {
                Ok(Ok(())) => self.books.write().retain(|book| book.id != id),
                Ok(Err(error)) => self.error(format!("Cannot remove this book: {error:#}")),
                Err(error) => self.error(error.to_string()),
            }
        });
    }

    fn add_bookmark(mut self, label: String, location: ReadingLocation) {
        let Some(session) = self.session.peek().clone() else {
            return;
        };
        let Some(library) = self.services.peek().library.clone() else {
            return;
        };
        let id = session.book.id;
        let expected = id.clone();
        spawn(async move {
            match tokio::task::spawn_blocking(move || library.add_bookmark(&id, &label, &location))
                .await
            {
                Ok(Ok(bookmark)) => {
                    if self
                        .session
                        .peek()
                        .as_ref()
                        .is_some_and(|s| s.book.id == expected)
                    {
                        self.bookmarks.write().push(bookmark);
                    }
                }
                Ok(Err(error)) => self.error(format!("Cannot save bookmark: {error:#}")),
                Err(error) => self.error(error.to_string()),
            }
        });
    }

    fn remove_bookmark(mut self, id: i64) {
        let Some(library) = self.services.peek().library.clone() else {
            return;
        };
        spawn(async move {
            match tokio::task::spawn_blocking(move || library.remove_bookmark(id)).await {
                Ok(Ok(())) => self.bookmarks.write().retain(|bookmark| bookmark.id != id),
                Ok(Err(error)) => self.error(format!("Cannot remove bookmark: {error:#}")),
                Err(error) => self.error(error.to_string()),
            }
        });
    }
}

#[component]
pub fn App() -> Element {
    let initial = use_context::<Services>();
    let mut state = State {
        services: use_signal(|| initial.clone()),
        books: use_signal(|| initial.books.clone()),
        preferences: use_signal(|| initial.preferences.clone()),
        session: use_signal(|| None),
        bookmarks: use_signal(Vec::new),
        settings: use_signal(|| false),
        message: use_signal(|| initial.error.clone()),
        jobs: use_signal(Vec::new),
        pending: use_signal(VecDeque::new),
        importing: use_signal(|| false),
        opening: use_signal(|| false),
        open_request: use_signal(|| 0),
        latest_location: use_signal(|| None),
    };
    crate::platform::use_file_drop(
        EventHandler::new(move |paths| state.enqueue(paths)),
        EventHandler::new(move |error| state.error(error)),
    );
    dioxus_native::use_window_event(move |event, _| {
        if matches!(
            event,
            dioxus_native::winit::event::WindowEvent::CloseRequested
        ) {
            state.services.peek().cancel.store(true, Ordering::Relaxed);
        }
    });
    use_future(move || async move {
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let error = state
                .services
                .peek()
                .persistence
                .as_ref()
                .and_then(Persistence::take_error);
            if let Some(error) = error {
                state.error(format!("Your changes could not be saved: {error}"));
            }
        }
    });
    use_hook(move || {
        let paths: Vec<_> = std::env::args_os().skip(1).map(PathBuf::from).collect();
        if !paths.is_empty() {
            spawn(async move {
                state.enqueue(paths);
            });
        }
    });
    let session = state.session.read().clone();
    let data_folder = initial
        .library
        .as_ref()
        .map(|lib| lib.root().display().to_string())
        .unwrap_or_default();

    rsx! {
        style { {include_str!("theme.css")} }
        div { class: "app-shell",
            onkeydown: move |event| {
                if event.modifiers().ctrl() && event.key().to_string().eq_ignore_ascii_case("o") { event.prevent_default(); state.choose_books(); }
                if event.key() == Key::Escape && *state.settings.peek() { state.settings.set(false); }
            },
            if let Some(session) = session {
                ReaderView {
                    key: "{session.book.id}", book: session.book, document: session.document,
                    preferences: (state.preferences)(), bookmarks: (state.bookmarks)(),
                    on_back: move |_| state.back_to_library(),
                    on_location: move |location| state.location(location),
                    on_preferences: move |prefs| state.preferences(prefs),
                    on_add_bookmark: move |(label, location)| state.add_bookmark(label, location),
                    on_remove_bookmark: move |id| state.remove_bookmark(id),
                    on_error: move |error| state.error(error),
                }
            } else {
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
                LibraryView { books: (state.books)(), on_open: move |id| state.open_book(id), on_import: move |_| state.choose_books(), on_settings: move |_| state.settings.set(true), on_remove: move |id| state.remove_book(id) }
                footer { class: "app-footer",
                    span { "Your books. Your quiet corner." }
                    span { "EPUB & MOBI  /  Stored on this computer" }
                }
            }
            if !state.jobs.read().is_empty() {
                section { class: "import-panel", style: "position:fixed;right:24px;bottom:24px;width:400px;max-height:320px;overflow:auto;margin:0;z-index:15;border:1px solid #d7cebb;",
                    div { style: "display:flex;justify-content:space-between;align-items:center;",
                        strong { if (state.importing)() { "Adding your books" } else { "Import results" } }
                        if (state.importing)() {
                            ActionButton { class: "button button-quiet", onpress: move |_| { state.services.peek().cancel.store(true, Ordering::Relaxed); }, "Cancel" }
                        } else {
                            ActionButton { class: "icon-button", aria_label: "Dismiss import results", onpress: move |_| state.jobs.clear(), "x" }
                        }
                    }
                    for (index, job) in (state.jobs)().into_iter().enumerate() {
                        div { key: "{index}", class: "import-job", style: "align-items:flex-start;",
                            div { style: "min-width:0;flex:1;", strong { "{job.state.filename}" } p { class: "muted", "{job.state.status}" }
                                if let Some(error) = job.state.error { p { style: "color:#8c473a;font-size:12px;overflow-wrap:anywhere;", "{error}" } }
                            }
                            if !(state.importing)() && (job.state.status == "Cancelled" || job.state.status.contains("fail") || job.state.status == "Could not import") {
                                ActionButton { class: "button button-quiet", onpress: move |_| state.enqueue(vec![job.path.clone()]), "Retry" }
                            }
                        }
                    }
                    if !(state.importing)() && state.jobs.read().iter().any(|job| job.state.error.is_some()) {
                        ActionButton { class: "button button-quiet", onpress: move |_| state.settings.set(true), "Import help" }
                    }
                }
            }
            if (state.opening)() {
                div { class: "modal-backdrop",
                    div { class: "modal", role: "status", h2 { "Opening your book..." } p { class: "muted", "Preparing its chapters and images." }
                        div { class: "modal-actions", ActionButton { class: "button button-quiet", onpress: move |_| { state.open_request += 1; state.opening.set(false); }, "Cancel" } }
                    }
                }
            }
            if (state.settings)() {
                Settings { data_folder, on_close: move |_| state.settings.set(false) }
            }
            if let Some(message) = (state.message)() {
                div { class: "toast", role: "alert", style: "z-index:40;display:flex;align-items:flex-start;gap:12px;",
                    span { "{message}" }
                    ActionButton { class: "icon-button", aria_label: "Dismiss message", onpress: move |_| state.message.set(None), "x" }
                }
            }
        }
    }
}
