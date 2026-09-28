use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};

use crate::{
    ebook::read_book,
    model::{BookRecord, Bookmark, ReaderPreferences, ReadingLocation},
};

static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Connections belong to individual operations, so the library can cross worker threads.
#[derive(Clone, Debug)]
pub struct Library {
    root: PathBuf,
}

struct Staging(PathBuf);

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl Library {
    pub fn open() -> Result<Self> {
        let root = match std::env::var_os("MOBI_READER_DATA_DIR") {
            Some(value) if !value.is_empty() => PathBuf::from(value),
            _ => directories::BaseDirs::new()
                .context("Cannot locate your local application data directory")?
                .data_local_dir()
                .join("MobiReader"),
        };
        Self::at(root)
    }

    pub fn at(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root).context("Cannot create the library directory")?;
        let library = Self {
            root: fs::canonicalize(root)?,
        };
        fs::create_dir_all(library.root.join("books"))?;
        fs::create_dir_all(library.root.join("staging"))?;
        library.managed_directory("books")?;
        library.managed_directory("staging")?;
        library.connection()?.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS books (id TEXT PRIMARY KEY, record TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS reading_state (
                 book_id TEXT PRIMARY KEY REFERENCES books(id) ON DELETE CASCADE,
                 location TEXT NOT NULL, last_read_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS preferences (id INTEGER PRIMARY KEY CHECK(id = 1), value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS bookmarks (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 book_id TEXT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
                 label TEXT NOT NULL, location TEXT NOT NULL, created_at TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS bookmarks_book ON bookmarks(book_id);"
        )?;
        Ok(library)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn managed_directory(&self, name: &str) -> Result<PathBuf> {
        let directory = self.root.join(name);
        let metadata = fs::symlink_metadata(&directory)?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "Managed library directory is not an ordinary directory"
        );
        ensure!(
            fs::canonicalize(&directory)?.parent() == Some(self.root.as_path()),
            "Managed library directory is outside the library"
        );
        Ok(directory)
    }

    fn connection(&self) -> Result<Connection> {
        let connection = Connection::open(self.root.join("library.sqlite3"))?;
        connection.busy_timeout(Duration::from_secs(10))?;
        connection.execute_batch("PRAGMA foreign_keys=ON;")?;
        Ok(connection)
    }

    pub fn list_books(&self) -> Result<Vec<BookRecord>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT books.record,reading_state.location,reading_state.last_read_at
             FROM books LEFT JOIN reading_state ON books.id=reading_state.book_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;
        let mut books = rows
            .map(|row| {
                let (record, location, last_read_at) = row?;
                decode_record(&record, location.as_deref(), last_read_at)
            })
            .collect::<Result<Vec<BookRecord>>>()?;
        books.sort_by(|a, b| {
            b.last_read_at
                .as_ref()
                .unwrap_or(&b.added_at)
                .cmp(a.last_read_at.as_ref().unwrap_or(&a.added_at))
        });
        Ok(books)
    }

    pub fn book_path(&self, id: &str) -> PathBuf {
        let component = if valid_id(id) { id } else { ".invalid-id" };
        self.root.join("books").join(component).join("book.epub")
    }

    pub fn load_preferences(&self) -> Result<ReaderPreferences> {
        let json: Option<String> = self
            .connection()?
            .query_row("SELECT value FROM preferences WHERE id=1", [], |row| {
                row.get(0)
            })
            .optional()?;
        json.map(|json| serde_json::from_str(&json).context("Saved preferences are damaged"))
            .transpose()
            .map(|preferences| preferences.unwrap_or_default())
    }

    pub fn save_preferences(&self, preferences: &ReaderPreferences) -> Result<()> {
        self.connection()?.execute(
            "INSERT INTO preferences(id,value) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET value=excluded.value",
            [serde_json::to_string(preferences)?],
        )?;
        Ok(())
    }

    pub fn save_location(&self, id: &str, location: &ReadingLocation) -> Result<()> {
        check_id(id)?;
        check_location(location)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Extract just the scalar in SQLite: frequent scroll saves neither deserialize
        // the cover nor write the large immutable book metadata again.
        let chapter_count: Option<i64> = transaction
            .query_row(
                "SELECT json_extract(record,'$.chapter_count') FROM books WHERE id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        let chapter_count = chapter_count.context("Book is no longer in the library")?;
        ensure!(
            chapter_count > 0 && (location.spine_index as u128) < chapter_count as u128,
            "Reading position is outside this book"
        );
        transaction.execute(
            "INSERT INTO reading_state(book_id,location,last_read_at) VALUES(?1,?2,?3)
             ON CONFLICT(book_id) DO UPDATE SET location=excluded.location,last_read_at=excluded.last_read_at",
            params![id, serde_json::to_string(location)?, Utc::now().to_rfc3339()],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn bookmarks(&self, id: &str) -> Result<Vec<Bookmark>> {
        check_id(id)?;
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT id,label,location,created_at FROM bookmarks WHERE book_id=?1 ORDER BY id",
        )?;
        let rows = statement.query_map([id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        rows.map(|row| {
            let (bookmark_id, label, location, created_at) = row?;
            Ok(Bookmark {
                id: bookmark_id,
                book_id: id.into(),
                label,
                location: serde_json::from_str(&location)?,
                created_at,
            })
        })
        .collect()
    }

    pub fn add_bookmark(
        &self,
        id: &str,
        label: &str,
        location: &ReadingLocation,
    ) -> Result<Bookmark> {
        check_id(id)?;
        check_location(location)?;
        let connection = self.connection()?;
        let record = get_record(&connection, id)?.context("Book is no longer in the library")?;
        ensure!(
            location.spine_index < record.chapter_count,
            "Bookmark is outside this book"
        );
        let created_at = Utc::now().to_rfc3339();
        connection.execute(
            "INSERT INTO bookmarks(book_id,label,location,created_at) VALUES(?1,?2,?3,?4)",
            params![id, label, serde_json::to_string(location)?, created_at],
        )?;
        Ok(Bookmark {
            id: connection.last_insert_rowid(),
            book_id: id.into(),
            label: label.into(),
            location: location.clone(),
            created_at,
        })
    }

    pub fn remove_bookmark(&self, bookmark_id: i64) -> Result<()> {
        self.connection()?
            .execute("DELETE FROM bookmarks WHERE id=?1", [bookmark_id])?;
        Ok(())
    }

    pub fn remove_book(&self, id: &str) -> Result<()> {
        check_id(id)?;
        self.managed_directory("books")?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure!(
            get_record(&transaction, id)?.is_some(),
            "Book is no longer in the library"
        );
        let directory = self.book_path(id).parent().unwrap().to_path_buf();
        let trash = self.new_staging()?;
        let moved = trash.0.join("removed");
        if directory.exists() {
            ensure!(
                !fs::symlink_metadata(&directory)?.file_type().is_symlink(),
                "Managed book directory is a symbolic link"
            );
            fs::rename(&directory, &moved).context("Cannot remove the managed book copy")?;
        }
        let result = (|| -> Result<()> {
            transaction.execute("DELETE FROM books WHERE id=?1", [id])?;
            transaction.commit()?;
            Ok(())
        })();
        if result.is_err() && moved.exists() {
            fs::rename(&moved, &directory)?;
        }
        result
    }

    fn new_staging(&self) -> Result<Staging> {
        let staging_root = self.managed_directory("staging")?;
        for _ in 0..100 {
            let name = format!(
                "{}-{}-{}",
                std::process::id(),
                Utc::now().timestamp_micros(),
                STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            );
            let path = staging_root.join(name);
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Staging(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        bail!("Cannot allocate an import staging directory")
    }

    pub fn import_book(&self, path: &Path, cancel: &AtomicBool) -> Result<BookRecord> {
        check_cancel(cancel)?;
        self.managed_directory("books")?;
        let format = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        ensure!(
            matches!(format.as_str(), "epub" | "mobi"),
            "Unsupported book format. Choose an EPUB or MOBI book"
        );
        ensure!(
            path.is_file(),
            "The selected book does not exist or is not a file"
        );
        let staging = self.new_staging()?;
        let normalized = staging.0.join("book.epub");
        let source_copy = if format == "epub" {
            normalized.clone()
        } else {
            staging.0.join(format!("source.{format}"))
        };
        let id = copy_and_hash(path, &source_copy, cancel)?;
        if let Some(record) = get_record(&self.connection()?, &id)? {
            ensure!(
                self.book_path(&id).is_file(),
                "The managed copy is missing. Remove this entry and import the original again"
            );
            return Ok(record);
        }
        if format != "epub" {
            crate::mobi::convert_to_epub(&source_copy, &normalized, cancel)?;
        }
        check_cancel(cancel)?;
        let document = read_book(&normalized)
            .context("This book could not be opened; it may be damaged or protected")?;
        ensure!(
            !document.chapters.is_empty(),
            "The book has no readable chapters"
        );
        check_cancel(cancel)?;
        let title = if document.metadata.title.trim().is_empty() {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Untitled book")
                .to_owned()
        } else {
            document.metadata.title
        };
        let record = BookRecord {
            id: id.clone(),
            title,
            author: document.metadata.author,
            format: format.to_uppercase(),
            added_at: Utc::now().to_rfc3339(),
            last_read_at: None,
            cover_data: document.metadata.cover_data,
            chapter_count: document.chapters.len(),
            location: ReadingLocation::default(),
        };
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = get_record(&transaction, &id)? {
            return Ok(existing);
        }
        check_cancel(cancel)?;
        let destination = self.book_path(&id).parent().unwrap().to_path_buf();
        ensure!(
            !destination.exists(),
            "A managed directory already exists for this book; no files were overwritten"
        );
        // Keep the copied MOBI alongside its normalized, validated EPUB.
        // For EPUB imports, book.epub already is the unchanged original copy.
        let publish = staging.0.join("publish");
        prepare_managed_copy(&normalized, &source_copy, &publish)?;
        fs::rename(&publish, &destination)?;
        let result = (|| -> Result<()> {
            check_cancel(cancel)?;
            transaction.execute(
                "INSERT INTO books(id,record) VALUES(?1,?2)",
                params![id, serde_json::to_string(&record)?],
            )?;
            transaction.commit()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&destination);
        }
        result?;
        Ok(record)
    }
}

fn valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn prepare_managed_copy(normalized: &Path, source_copy: &Path, publish: &Path) -> Result<()> {
    fs::create_dir(publish)?;
    fs::rename(normalized, publish.join("book.epub"))?;
    if source_copy != normalized {
        fs::rename(source_copy, publish.join("source.mobi"))?;
    }
    Ok(())
}
fn check_id(id: &str) -> Result<()> {
    ensure!(valid_id(id), "Invalid library book ID");
    Ok(())
}
fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    ensure!(!cancel.load(Ordering::Acquire), "Import cancelled");
    Ok(())
}
fn check_location(location: &ReadingLocation) -> Result<()> {
    ensure!(
        location.block_fraction.is_finite()
            && (0.0..=1.0).contains(&location.block_fraction)
            && location.chapter_fraction.is_finite()
            && (0.0..=1.0).contains(&location.chapter_fraction),
        "Invalid reading position"
    );
    Ok(())
}
fn get_record(connection: &Connection, id: &str) -> Result<Option<BookRecord>> {
    let row: Option<(String, Option<String>, Option<String>)> = connection
        .query_row(
            "SELECT books.record,reading_state.location,reading_state.last_read_at
         FROM books LEFT JOIN reading_state ON books.id=reading_state.book_id WHERE books.id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    row.map(|(record, location, last_read_at)| {
        decode_record(&record, location.as_deref(), last_read_at)
    })
    .transpose()
}

fn decode_record(
    json: &str,
    location: Option<&str>,
    last_read_at: Option<String>,
) -> Result<BookRecord> {
    let mut record: BookRecord = serde_json::from_str(json).context("Library entry is damaged")?;
    // A legacy entry keeps its original saved position until its first new save.
    if let Some(location) = location {
        record.location =
            serde_json::from_str(location).context("Saved reading position is damaged")?;
        record.last_read_at = last_read_at;
    }
    Ok(record)
}
fn copy_and_hash(source: &Path, destination: &Path, cancel: &AtomicBool) -> Result<String> {
    let mut input = File::open(source).context("Cannot read the selected book")?;
    let mut output = File::create(destination)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        check_cancel(cancel)?;
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        output.write_all(&buffer[..count])?;
        digest.update(&buffer[..count]);
        size += count as u64;
    }
    ensure!(size > 0, "The selected book is empty");
    output.sync_all()?;
    check_cancel(cancel)?;
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use zip::{ZipWriter, write::SimpleFileOptions};

    fn library() -> (TempDir, Library) {
        let temp = TempDir::new().unwrap();
        let library = Library::at(temp.path().join("library")).unwrap();
        (temp, library)
    }

    fn fixture(path: &Path) {
        let mut zip = ZipWriter::new(File::create(path).unwrap());
        for (name, content) in [
            ("mimetype", "application/epub+zip"),
            (
                "META-INF/container.xml",
                r#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
            ),
            (
                "content.opf",
                r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="book-id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="book-id">test-book</dc:identifier><dc:title>Test Book</dc:title><dc:creator>Test Author</dc:creator><dc:language>en</dc:language><meta property="dcterms:modified">2026-09-27T00:00:00Z</meta></metadata><manifest><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/></manifest><spine><itemref idref="chapter"/></spine></package>"#,
            ),
            (
                "chapter.xhtml",
                r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>One</title></head><body><h1 id="one">One</h1><p id="paragraph">A book that stays on this computer.</p></body></html>"#,
            ),
            (
                "nav.xhtml",
                r#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><head><title>Contents</title></head><body><nav epub:type="toc"><ol><li><a href="chapter.xhtml">One</a></li></ol></nav></body></html>"#,
            ),
        ] {
            zip.start_file(
                name,
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn preferences_positions_and_bookmarks_survive_reopening() {
        let (_temp, library) = library();
        assert_eq!(
            library.load_preferences().unwrap(),
            ReaderPreferences::default()
        );
        let preferences = ReaderPreferences {
            font_size: 24,
            theme: "night".into(),
            ..ReaderPreferences::default()
        };
        library.save_preferences(&preferences).unwrap();
        let id = "a".repeat(64);
        let record = BookRecord {
            id: id.clone(),
            title: "Stored book".into(),
            author: "Author".into(),
            format: "EPUB".into(),
            added_at: Utc::now().to_rfc3339(),
            last_read_at: None,
            cover_data: None,
            chapter_count: 3,
            location: ReadingLocation::default(),
        };
        library
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO books(id,record) VALUES(?1,?2)",
                params![id, serde_json::to_string(&record).unwrap()],
            )
            .unwrap();
        let location = ReadingLocation {
            spine_index: 1,
            block_id: "paragraph".into(),
            block_fraction: 0.25,
            chapter_fraction: 0.5,
        };
        library.save_location(&id, &location).unwrap();
        let bookmark = library.add_bookmark(&id, "Return here", &location).unwrap();
        let reopened = Library::at(library.root.clone()).unwrap();
        assert_eq!(reopened.load_preferences().unwrap(), preferences);
        assert_eq!(reopened.list_books().unwrap()[0].location, location);
        assert!(reopened.list_books().unwrap()[0].last_read_at.is_some());
        assert_eq!(reopened.bookmarks(&id).unwrap(), vec![bookmark.clone()]);
        reopened.remove_bookmark(bookmark.id).unwrap();
        assert!(reopened.bookmarks(&id).unwrap().is_empty());
        reopened.add_bookmark(&id, "Cascade", &location).unwrap();
        reopened.remove_book(&id).unwrap();
        assert!(reopened.list_books().unwrap().is_empty());
        assert!(reopened.bookmarks(&id).unwrap().is_empty());
    }

    #[test]
    fn reading_state_overlays_legacy_positions_without_rewriting_the_cover() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("legacy-library");
        fs::create_dir(&root).unwrap();
        let connection = Connection::open(root.join("library.sqlite3")).unwrap();
        connection
            .execute_batch("CREATE TABLE books(id TEXT PRIMARY KEY, record TEXT NOT NULL);")
            .unwrap();
        let id = "b".repeat(64);
        let old_location = ReadingLocation {
            spine_index: 1,
            block_id: "legacy-paragraph".into(),
            chapter_fraction: 0.4,
            ..ReadingLocation::default()
        };
        let cover = format!("data:image/png;base64,{}", "A".repeat(400_000));
        let old_record = BookRecord {
            id: id.clone(),
            title: "Legacy book".into(),
            author: "Test author".into(),
            format: "EPUB".into(),
            added_at: "2026-09-27T00:00:00Z".into(),
            last_read_at: Some("2026-09-27T01:00:00Z".into()),
            cover_data: Some(cover.clone()),
            chapter_count: 3,
            location: old_location.clone(),
        };
        let original_json = serde_json::to_string(&old_record).unwrap();
        connection
            .execute(
                "INSERT INTO books(id,record) VALUES(?1,?2)",
                params![id, original_json],
            )
            .unwrap();
        // Any accidental metadata rewrite makes this test fail immediately.
        connection.execute_batch("CREATE TRIGGER immutable_book_record BEFORE UPDATE OF record ON books BEGIN SELECT RAISE(FAIL, 'book metadata was rewritten'); END;").unwrap();
        drop(connection);
        let library = Library::at(root.clone()).unwrap();
        assert_eq!(library.list_books().unwrap()[0], old_record);
        let mut latest = old_location;
        for index in 0..8 {
            latest = ReadingLocation {
                spine_index: 2,
                block_id: format!("new-paragraph-{index}"),
                chapter_fraction: index as f64 / 8.0,
                ..ReadingLocation::default()
            };
            library.save_location(&id, &latest).unwrap();
        }
        let reopened = Library::at(root).unwrap();
        let record = reopened.list_books().unwrap().remove(0);
        assert_eq!(record.location, latest);
        assert_eq!(record.cover_data, Some(cover));
        assert_ne!(record.last_read_at, old_record.last_read_at);
        assert_eq!(
            get_record(&reopened.connection().unwrap(), &id)
                .unwrap()
                .unwrap(),
            record
        );
        let stored_json: String = reopened
            .connection()
            .unwrap()
            .query_row("SELECT record FROM books WHERE id=?1", [&id], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(stored_json, original_json);
        reopened.remove_book(&id).unwrap();
        let count: i64 = reopened
            .connection()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM reading_state", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn import_copies_deduplicates_and_preserves_original() {
        let (temp, library) = library();
        let source = temp.path().join("original.epub");
        fixture(&source);
        let original = fs::read(&source).unwrap();
        let cancel = AtomicBool::new(false);
        let record = library.import_book(&source, &cancel).unwrap();
        let same_bytes = temp.path().join("renamed.epub");
        fs::copy(&source, &same_bytes).unwrap();
        let duplicate = library.import_book(&same_bytes, &cancel).unwrap();
        assert_eq!(record.id, duplicate.id);
        assert_eq!(record.title, "Test Book");
        assert_eq!(library.list_books().unwrap().len(), 1);
        assert_eq!(fs::read(library.book_path(&record.id)).unwrap(), original);
        assert_eq!(
            fs::read_dir(library.root.join("staging")).unwrap().count(),
            0
        );
        library.remove_book(&record.id).unwrap();
        assert!(!library.book_path(&record.id).exists());
        assert_eq!(fs::read(&source).unwrap(), original);
        assert!(same_bytes.exists());
    }

    #[test]
    fn managed_mobi_copy_retains_original_and_normalized_epub() {
        let temp = TempDir::new().unwrap();
        let original = temp.path().join("original.mobi");
        fs::write(&original, b"synthetic original MOBI bytes").unwrap();
        let staged_source = temp.path().join("source.mobi");
        fs::copy(&original, &staged_source).unwrap();
        let normalized = temp.path().join("book.epub");
        // A controlled valid conversion output exercises managed publication.
        fixture(&normalized);
        let epub_bytes = fs::read(&normalized).unwrap();
        let publish = temp.path().join("publish");
        prepare_managed_copy(&normalized, &staged_source, &publish).unwrap();
        assert_eq!(
            fs::read(publish.join("source.mobi")).unwrap(),
            fs::read(&original).unwrap()
        );
        assert_eq!(fs::read(publish.join("book.epub")).unwrap(), epub_bytes);
        assert_eq!(
            read_book(&publish.join("book.epub"))
                .unwrap()
                .metadata
                .title,
            "Test Book"
        );
        assert!(original.exists());
    }

    #[test]
    fn cancellation_and_damaged_books_leave_no_published_files() {
        let (temp, library) = library();
        let source = temp.path().join("damaged.epub");
        fs::write(&source, b"not an epub").unwrap();
        let error = library
            .import_book(&source, &AtomicBool::new(true))
            .unwrap_err();
        assert!(error.to_string().contains("cancelled"));
        assert!(
            library
                .import_book(&source, &AtomicBool::new(false))
                .is_err()
        );
        let empty = temp.path().join("empty.epub");
        fs::write(&empty, []).unwrap();
        assert!(
            library
                .import_book(&empty, &AtomicBool::new(false))
                .is_err()
        );
        let unsupported = temp.path().join("document.pdf");
        fs::write(&unsupported, b"pdf").unwrap();
        assert!(
            library
                .import_book(&unsupported, &AtomicBool::new(false))
                .is_err()
        );
        assert!(library.list_books().unwrap().is_empty());
        assert_eq!(
            fs::read_dir(library.root.join("staging")).unwrap().count(),
            0
        );
        assert_eq!(fs::read_dir(library.root.join("books")).unwrap().count(), 0);
        assert!(source.exists());
    }

    #[test]
    fn invalid_ids_and_locations_cannot_escape_managed_storage() {
        let (_temp, library) = library();
        for id in ["../outside", "C:\\outside", "", "/outside", "ABCDEF"] {
            assert!(library.remove_book(id).is_err());
            assert!(
                library
                    .save_location(id, &ReadingLocation::default())
                    .is_err()
            );
            assert!(library.bookmarks(id).is_err());
            assert!(
                library
                    .add_bookmark(id, "bad", &ReadingLocation::default())
                    .is_err()
            );
            assert!(
                library
                    .book_path(id)
                    .starts_with(library.root.join("books"))
            );
        }
        assert!(
            check_location(&ReadingLocation {
                block_fraction: f64::NAN,
                ..ReadingLocation::default()
            })
            .is_err()
        );
        assert!(
            check_location(&ReadingLocation {
                chapter_fraction: 1.1,
                ..ReadingLocation::default()
            })
            .is_err()
        );
    }

    #[test]
    fn built_in_mobi_import_deduplicates_and_preserves_both_managed_copies() {
        let (temp, library) = library();
        let source = temp.path().join("original.MOBI");
        let original = crate::mobi_decode::fixture();
        fs::write(&source, &original).unwrap();
        let cancel = AtomicBool::new(false);
        let record = library.import_book(&source, &cancel).unwrap();
        assert_eq!(record.title, "Native MOBI Fixture");
        assert_eq!(record.author, "Mobi Reader Test Studio");
        assert_eq!(record.format, "MOBI");
        assert!(record.chapter_count > 0);
        let normalized = library.book_path(&record.id);
        let managed_original = normalized.parent().unwrap().join("source.mobi");
        assert_eq!(fs::read(&managed_original).unwrap(), original);
        let document = read_book(&normalized).unwrap();
        assert_eq!(document.metadata.title, record.title);
        assert!(
            document
                .chapters
                .iter()
                .flat_map(|chapter| &chapter.blocks)
                .any(|block| !block.text.trim().is_empty())
        );
        let renamed = temp.path().join("renamed.mobi");
        fs::copy(&source, &renamed).unwrap();
        let duplicate = library.import_book(&renamed, &cancel).unwrap();
        assert_eq!(duplicate.id, record.id);
        assert_eq!(library.list_books().unwrap().len(), 1);
        assert_eq!(
            fs::read_dir(library.root.join("staging")).unwrap().count(),
            0
        );
        library.remove_book(&record.id).unwrap();
        assert!(!normalized.exists());
        assert!(!managed_original.exists());
        assert_eq!(fs::read(&source).unwrap(), original);
        assert_eq!(fs::read(&renamed).unwrap(), original);
    }

    #[test]
    fn cancelled_and_damaged_mobi_imports_leave_no_library_files() {
        let (temp, library) = library();
        let original = crate::mobi_decode::fixture();
        let source = temp.path().join("cancelled.mobi");
        fs::write(&source, &original).unwrap();
        let error = library
            .import_book(&source, &AtomicBool::new(true))
            .unwrap_err();
        assert!(error.to_string().to_lowercase().contains("cancel"));
        let damaged = temp.path().join("damaged.mobi");
        fs::write(&damaged, &original[..78]).unwrap();
        assert!(
            library
                .import_book(&damaged, &AtomicBool::new(false))
                .is_err()
        );
        assert!(library.list_books().unwrap().is_empty());
        assert_eq!(
            fs::read_dir(library.root.join("staging")).unwrap().count(),
            0
        );
        assert_eq!(fs::read_dir(library.root.join("books")).unwrap().count(), 0);
        assert_eq!(fs::read(&source).unwrap(), original);
        assert!(damaged.exists());
    }
}
