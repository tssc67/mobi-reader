use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ReadingLocation {
    pub spine_index: usize,
    pub block_id: String,
    pub block_fraction: f64,
    pub chapter_fraction: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BookRecord {
    pub id: String,
    pub title: String,
    pub author: String,
    pub format: String,
    pub added_at: String,
    pub last_read_at: Option<String>,
    pub cover_data: Option<String>,
    pub chapter_count: usize,
    pub location: ReadingLocation,
}

impl BookRecord {
    pub fn progress(&self) -> f64 {
        if self.chapter_count == 0 {
            return 0.0;
        }
        ((self.location.spine_index as f64 + self.location.chapter_fraction)
            / self.chapter_count as f64)
            .clamp(0.0, 1.0)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BookMetadata {
    pub title: String,
    pub author: String,
    pub cover_data: Option<String>,
    pub chapter_count: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TocEntry {
    pub title: String,
    pub spine_index: usize,
    pub fragment: Option<String>,
    pub depth: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContentBlock {
    pub id: String,
    pub html: String,
    pub text: String,
    pub source_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Chapter {
    pub title: String,
    pub spine_index: usize,
    pub href: String,
    pub blocks: Vec<ContentBlock>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BookDocument {
    pub metadata: BookMetadata,
    pub toc: Vec<TocEntry>,
    pub chapters: Vec<Chapter>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchHit {
    pub spine_index: usize,
    pub block_id: String,
    pub chapter_title: String,
    pub snippet: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bookmark {
    pub id: i64,
    pub book_id: String,
    pub label: String,
    pub location: ReadingLocation,
    pub created_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReaderPreferences {
    pub font_size: u16,
    pub line_height: f64,
    pub column_width: u16,
    pub serif: bool,
    pub theme: String,
}

impl Default for ReaderPreferences {
    fn default() -> Self {
        Self {
            font_size: 20,
            line_height: 1.65,
            column_width: 680,
            serif: true,
            theme: "paper".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImportJob {
    pub filename: String,
    pub status: String,
    pub error: Option<String>,
}
