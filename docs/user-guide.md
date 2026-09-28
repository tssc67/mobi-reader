# Mobi Reader

A local Windows ebook library and chapter reader written in Rust. The interface uses Dioxus Native with Blitz layout and Vello/wgpu rendering. It runs as a native window; it does not need WebView2, a browser server, or the Dioxus CLI.

## What it supports

- Reflowable EPUB 2 and EPUB 3, including NCX/HTML contents, nested navigation, chapter fragments, metadata, covers, and local raster images.
- `.mobi` import through an independent built-in converter to EPUB, with no external software required. Original MOBI files are preserved.
- A managed local library with original files preserved, duplicate detection by source-file SHA-256, title/author filtering, recent/title sorting, and reading/finished shelves.
- File picker, window file drops, and file paths supplied on the command line. Batch imports show progress, errors, cancellation, and retry actions.
- Scrollable chapters, previous/next chapter controls, contents navigation, case-insensitive Unicode book search, and bookmarks.
- Persisted reading locations and appearance preferences: text size, line spacing, column width, Source Serif 4/Source Sans 3, and paper/sepia/night colors.

Reading and library storage are offline. Book scripts, publisher CSS, event handlers, external resources, and external link destinations are removed during normalization. Headings, emphasis, lists, internal links, simple tables, and supported local images retain their semantics. This reader does not reproduce publisher page design.

## Build and run on Windows

Use Rust **1.96 or newer** with the `x86_64-pc-windows-msvc` toolchain and Visual Studio Build Tools with the C++ desktop workload and Windows SDK. The native renderer also needs a working wgpu-compatible graphics driver. The first build downloads Cargo dependencies, including the pinned native renderer repository.

From PowerShell in the repository:

```powershell
cargo run
cargo test --all-targets
cargo build --release
.\target\release\mobi-reader.exe
```

Import files when starting the application:

```powershell
cargo run -- "C:\Books\novel.epub" "C:\Books\collection.mobi"
.\target\release\mobi-reader.exe "C:\Books\novel.epub"
```

The executable embeds its application styles and five font files. It does not load those assets from the repository at runtime. Keep the font license notices with any distribution.

The original open-book icon lives in `assets/icon.svg`. `build.rs` generates nine icon sizes (16-256 px), embeds them as Windows executable resources, and supplies the matching title-bar/taskbar icon. No external icon file is needed beside the executable.

`dioxus-native` is pinned to Blitz commit `7931e6794d5d0e791660cc3474536047c01239ed` in `Cargo.toml`; Dioxus core packages use 0.7.10. Keep `Cargo.lock` for reproducible dependency resolution. No `dx` command is needed.

## Built-in MOBI import

The reader parses and converts common DRM-free legacy MOBI books directly, including uncompressed, PalmDOC, and HUFF/CDIC compressed text. Dual-format MOBI files use their legacy content. UTF-8 and Windows-1252 text, titles, authors, covers, local raster images, and internal `filepos` links are supported. Page breaks provide sections and contents navigation.

Conversion runs on a background worker and produces a temporary EPUB, which is validated before the book is published to the library. The original MOBI file stays untouched and is also preserved as a managed source copy. Pure KF8 files, DRM-protected content, and fixed-layout books are unsupported; use a reflowable EPUB edition instead.

Actual Project Gutenberg legacy MOBI books 11, 55, and 19033 have been imported successfully. The illustrated book 19033 produced 54 image blocks and 43 blocks with internal links. Synthetic tests also exercise HUFF/CDIC decoding. These checks do not establish comprehensive format parity with Calibre or coverage of every MOBI variant.

To check a book through the import pipeline without opening the GUI:

```powershell
cargo run --example verify_import -- "C:\Books\book.mobi"
```

The example uses a temporary library. Pass `-` instead of a path to read MOBI bytes from standard input.

## Local storage

The default Windows library folder is `%LOCALAPPDATA%\MobiReader`. Settings displays the resolved location. It contains:

- `library.sqlite3` and SQLite journal files for library entries, preferences, reading locations, and bookmarks.
- `books\<source-sha256>\book.epub` for validated managed copies.
- `books\<source-sha256>\source.mobi` for the preserved source copy of an imported MOBI book.
- `staging` for temporary imports and converter output, normally cleaned when an import finishes or fails.

To keep development data separate, set the override before launching:

```powershell
$env:MOBI_READER_DATA_DIR = 'C:\ReaderData\Development'
cargo run
```

The override selects another library; it does not move an existing one. Removing a library entry deletes its managed copy and bookmarks, while preserving the original imported file. Back up the whole library folder while the application is closed if you want to preserve your collection and reading state.

## Developer fixture

Generate an original synthetic EPUB with chapters, nested contents, internal links, images, tables, and Unicode text:

```powershell
cargo run --example make_fixture
cargo run -- .\target\fixtures\reading-room.epub
```

The generator uses no downloaded book content. Tests exercise EPUB 2/3 parsing, built-in MOBI decoding and conversion, sanitization, Unicode search, storage, duplicate imports, cancellation cleanup, bookmarks, and ordered persistence. Automated backend checks do not establish that every native GUI interaction works; full GUI coverage remains incomplete.

The `native_preview` example provides offscreen rendering checks through the native layout/rendering backend without driving a desktop GUI. These checks validate rendered output and layout; they do not replace interactive checks of file dialogs, focus, scrolling, or window events.

## Startup measurement

Windows shows an animated ivory-and-amber loading layer inside the main reader window while the native renderer initializes. This native child layer is painted on a separate thread, so its animation continues while wgpu starts behind it. It is removed after an active reader redraw, with no artificial minimum display time and no separate splash window. It provides feedback during rendering startup; it cannot cover Windows executable loading or the brief work before the main window exists.

Use `cargo run --example profile_startup -- --preview-loading` to preview the layer with a simulated three-second initialization delay. This delay is only in the diagnostic example, never the reader executable.

Run `cargo run --release --example profile_startup` to measure library loading, font registration, application mount, and the redraw after the first frame. It briefly opens the real application and closes automatically. Timings start inside the process; they exclude Windows executable loading and are not a measurement of display presentation. Compare repeated launches after builds finish. Cold driver caches, disk caches, and system load can change the result.

On the development machine, a warm empty-library launch reached the redraw after the first frame in about 1.02 seconds; library and font setup took about 12 milliseconds. A Vello Hybrid comparison measured 0.97-1.00 seconds across three idle warm launches, which did not establish a meaningful improvement, so the original Vello backend is retained.

## Current limits

Protected/encrypted book content and fixed-layout EPUBs are unsupported. Known font obfuscation is accepted, but publisher fonts are not loaded. SVG/vector artwork, audio, video, scripting, publisher styles, and remote images are omitted. Supported raster images are PNG, JPEG, GIF, WebP, AVIF, and BMP, subject to native decoder support.

The reader displays one scrollable chapter at a time, rather than paginated spreads. Reading positions use semantic block anchors and fractions; layout changes can make restoration approximate. Search returns at most one result per block and caps results at 500; it uses Unicode lowercasing without accent-insensitive or linguistic matching.

EPUB extraction limits are 8 MiB per chapter/image resource, 128 MiB of chapter source text, 64 MiB of image bytes, 10,000 chapters, and chapter markup depth of 128. Built-in MOBI import accepts at most 256 MiB of input, 64 MiB of decoded text, 8 MiB per image, and 64 MiB of images in total. Emitted markup is limited to 7 MiB per section to fit the EPUB reader's 8 MiB resource cap. Deep or excessively complex markup is bounded, and oversized books fail with an error. Opening cancellation discards the pending result rather than interrupting an EPUB parse already running; import cancellation is checked between parsing and publication.

There is no cloud sync, annotation/highlight editor, PDF reader, AZW/KFX import, or library relocation interface.

## License

Application source, including the independently implemented MOBI decoder and converter, is MIT licensed; see [LICENSE](../LICENSE). No Calibre code was copied or adapted. The bundled Adobe Source fonts retain their SIL Open Font License 1.1 notices in [Source Serif 4 license](../assets/fonts/SourceSerif4-LICENSE.md) and [Source Sans 3 license](../assets/fonts/SourceSans3-LICENSE.md). Dependencies retain their respective licenses.
