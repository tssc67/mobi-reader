//! One ordered worker serializes reader writes without blocking event handlers.
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, mpsc},
};

use anyhow::{Context, Result, anyhow};

use crate::{
    library::Library,
    model::{ReaderPreferences, ReadingLocation},
};

enum Command {
    Location(String, ReadingLocation),
    Preferences(ReaderPreferences),
    Flush(mpsc::Sender<std::result::Result<(), String>>),
}

#[derive(Clone)]
pub struct Persistence {
    sender: mpsc::Sender<Command>,
    errors: Arc<Mutex<VecDeque<String>>>,
}

impl Persistence {
    pub fn new(library: Library) -> Self {
        let (sender, receiver) = mpsc::channel();
        let errors = Arc::new(Mutex::new(VecDeque::new()));
        let worker_errors = errors.clone();
        let spawned = std::thread::Builder::new()
            .name("reader-persistence".into())
            .spawn(move || {
                let mut unflushed_errors = Vec::new();
                while let Ok(command) = receiver.recv() {
                    let result = match command {
                        Command::Location(id, location) => library
                            .save_location(&id, &location)
                            .context("Cannot save your reading position"),
                        Command::Preferences(preferences) => library
                            .save_preferences(&preferences)
                            .context("Cannot save reader settings"),
                        Command::Flush(reply) => {
                            let result = if unflushed_errors.is_empty() {
                                Ok(())
                            } else {
                                Err(unflushed_errors.join("\n"))
                            };
                            unflushed_errors.clear();
                            let _ = reply.send(result);
                            continue;
                        }
                    };
                    if let Err(error) = result {
                        let message = format!("{error:#}");
                        unflushed_errors.push(message.clone());
                        worker_errors
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push_back(message);
                    }
                }
            });
        if let Err(error) = spawned {
            errors
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push_back(format!("Cannot start the persistence worker: {error}"));
        }
        Self { sender, errors }
    }

    /// A successful result means queued; `flush` confirms that SQLite accepted it.
    pub fn save_location(&self, id: String, location: ReadingLocation) -> Result<()> {
        self.sender
            .send(Command::Location(id, location))
            .map_err(|_| anyhow!("The persistence worker stopped; reading position was not queued"))
    }

    pub fn save_preferences(&self, preferences: ReaderPreferences) -> Result<()> {
        self.sender
            .send(Command::Preferences(preferences))
            .map_err(|_| anyhow!("The persistence worker stopped; reader settings were not queued"))
    }

    /// FIFO barrier. Reports all failed writes since the previous barrier, including
    /// failures that have already been retrieved with `take_error` for display.
    pub fn flush(&self) -> Result<()> {
        let (reply, result) = mpsc::channel();
        self.sender.send(Command::Flush(reply)).map_err(|_| {
            anyhow!("The persistence worker stopped before pending saves could be confirmed")
        })?;
        result
            .recv()
            .context("The persistence worker stopped while confirming pending saves")?
            .map_err(|message| anyhow!(message))
    }

    pub fn take_error(&self) -> Option<String> {
        self.errors
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::BookRecord;
    use rusqlite::{Connection, params};
    use tempfile::TempDir;

    fn library_with_book() -> (TempDir, Library, String) {
        let temp = TempDir::new().unwrap();
        let library = Library::at(temp.path().join("library")).unwrap();
        let id = "a".repeat(64);
        let book = BookRecord {
            id: id.clone(),
            title: "Worker test".into(),
            author: "Developer fixture".into(),
            format: "EPUB".into(),
            added_at: "2026-09-28T00:00:00Z".into(),
            last_read_at: None,
            cover_data: None,
            chapter_count: 5,
            location: ReadingLocation::default(),
        };
        Connection::open(library.root().join("library.sqlite3"))
            .unwrap()
            .execute(
                "INSERT INTO books(id,record) VALUES(?1,?2)",
                params![id, serde_json::to_string(&book).unwrap()],
            )
            .unwrap();
        (temp, library, id)
    }

    #[test]
    fn cloned_handles_share_one_fifo_worker_and_flush_waits_for_the_latest_save() {
        let (_temp, library, id) = library_with_book();
        let persistence = Persistence::new(library.clone());
        let clone = persistence.clone();
        for index in 0..40 {
            let location = ReadingLocation {
                spine_index: index % 5,
                block_id: format!("paragraph-{index}"),
                chapter_fraction: index as f64 / 40.0,
                ..ReadingLocation::default()
            };
            let handle = if index % 2 == 0 { &persistence } else { &clone };
            handle.save_location(id.clone(), location).unwrap();
            handle
                .save_preferences(ReaderPreferences {
                    font_size: 16 + index as u16,
                    ..ReaderPreferences::default()
                })
                .unwrap();
        }
        clone.flush().unwrap();
        let book = library.list_books().unwrap().remove(0);
        assert_eq!(book.location.spine_index, 4);
        assert_eq!(book.location.block_id, "paragraph-39");
        assert_eq!(library.load_preferences().unwrap().font_size, 55);
        assert!(persistence.take_error().is_none());
    }

    #[test]
    fn flush_reports_write_failures_and_the_worker_can_continue() {
        let (_temp, library, id) = library_with_book();
        let persistence = Persistence::new(library.clone());
        persistence
            .save_location("../invalid".into(), ReadingLocation::default())
            .unwrap();
        let error = persistence.flush().unwrap_err().to_string();
        assert!(error.contains("Invalid library book ID"));
        assert!(
            persistence
                .take_error()
                .unwrap()
                .contains("Invalid library book ID")
        );
        persistence
            .save_location(
                id,
                ReadingLocation {
                    block_id: "recovered".into(),
                    ..ReadingLocation::default()
                },
            )
            .unwrap();
        persistence.flush().unwrap();
        assert_eq!(
            library.list_books().unwrap()[0].location.block_id,
            "recovered"
        );
        assert!(persistence.take_error().is_none());
    }

    #[test]
    fn displaying_a_failure_does_not_hide_it_from_the_flush_barrier() {
        let (_temp, library, _id) = library_with_book();
        let persistence = Persistence::new(library);
        persistence
            .save_location("bad".into(), ReadingLocation::default())
            .unwrap();
        // A failed write must not prevent later queued commands from being processed.
        persistence
            .save_preferences(ReaderPreferences::default())
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let visible_error = loop {
            if let Some(error) = persistence.take_error() {
                break error;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Persistence error was never published"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert!(visible_error.contains("Invalid library book ID"));
        assert!(persistence.flush().is_err());
        assert!(persistence.flush().is_ok());
    }
}
