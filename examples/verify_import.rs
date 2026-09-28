//! Import a real book into a temporary library without opening the GUI.
//! Pass a local path, or '-' to read MOBI bytes from standard input.
use anyhow::{Context, Result, ensure};
use mobi_reader::{ebook::read_book, library::Library};
use std::{io::Read, path::PathBuf, sync::atomic::AtomicBool};

fn main() -> Result<()> {
    let argument = std::env::args_os()
        .nth(1)
        .context("Pass a book path, or '-' for MOBI on stdin")?;
    let temp = tempfile::TempDir::new()?;
    let source = if argument == "-" {
        let source = temp.path().join("stdin.mobi");
        let mut input = std::io::stdin().lock().take(256 * 1024 * 1024 + 1);
        let mut file = std::fs::File::create(&source)?;
        ensure!(
            std::io::copy(&mut input, &mut file)? <= 256 * 1024 * 1024,
            "Input exceeds 256 MiB"
        );
        source
    } else {
        PathBuf::from(argument)
    };
    let library = Library::at(temp.path().join("library"))?;
    let record = library.import_book(&source, &AtomicBool::new(false))?;
    let document = read_book(&library.book_path(&record.id))?;
    let blocks = document
        .chapters
        .iter()
        .flat_map(|chapter| &chapter.blocks)
        .collect::<Vec<_>>();
    println!("{} / {}", record.title, record.author);
    println!(
        "{} chapters, {} blocks, {} text bytes, {} image blocks, {} internal-link blocks, cover: {}",
        document.chapters.len(),
        blocks.len(),
        blocks.iter().map(|b| b.text.len()).sum::<usize>(),
        blocks
            .iter()
            .filter(|b| b.html.contains("data:image/"))
            .count(),
        blocks
            .iter()
            .filter(|b| b.html.contains("reader://chapter/"))
            .count(),
        record.cover_data.is_some()
    );
    Ok(())
}
