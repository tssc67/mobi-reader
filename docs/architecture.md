# Architecture

Mobi Reader is a Windows and Linux desktop application. Dioxus manages the UI; Blitz handles
HTML/CSS layout; Vello and wgpu render a native window. There is no web server or
WebView2 runtime. Styles, fonts, and the application icon are embedded.

## Source map

| Module | Responsibility |
| --- | --- |
| `src/main.rs`, `src/splash.rs` | Runtime, fonts, native window, startup loading layer |
| `src/app.rs` | Application state, import jobs, library/reader transitions |
| `src/library_view.rs`, `src/theme.css` | Library interface and shared visual styles |
| `src/reader.rs`, `src/viewport.rs` | Reading UI, navigation, scroll-position capture/restoration |
| `src/ebook.rs` | EPUB parsing, normalized chapter content, resource handling and search |
| `src/mobi_decode.rs`, `src/mobi.rs` | Legacy MOBI decoding and independent EPUB conversion |
| `src/library.rs` | Managed book files, SQLite library and import publication |
| `src/persistence.rs` | Ordered background persistence and flush/error handling |
| `src/model.rs`, `src/settings.rs` | Shared records and appearance settings |
| `src/controls.rs`, `src/platform.rs` | Shared native controls and platform integration |
| `build.rs`, `assets/icon.svg` | Window icon generation and Windows executable resources |

## Import and reading

Imports run on a background worker. A source-file hash identifies duplicates.
Legacy MOBI is decoded into a temporary EPUB; EPUB content is parsed and normalized
before a managed library copy is published. The original user file is preserved.
The managed MOBI source is retained alongside its normalized EPUB.

Book markup is treated as untrusted content. Scripts, event handlers, publisher
CSS and external resources are removed. Resource and expansion limits constrain
parsing. The reader presents semantic blocks in scrollable chapters, with saved
block anchors and fractions rather than fixed page numbers.

## Renderer patch

The native stack is pinned in Cargo.toml and Cargo.lock. Only `blitz-dom` is
vendored. Its local patch preserves Parley's retained text layout during intrinsic
measurement, preventing cached box sizes from being painted with another width's
line breaks. Existing layout caches remain enabled. Uncached measurements incur
a temporary text-layout clone. See the vendor README before upgrading Blitz.

## Verification

Unit tests cover parsing, conversion, sanitization, storage and reading-state
behavior. `examples/native_preview.rs` uses the real library components and native
renderer to check exact text line ranges across resize round trips at several
display scales, then optionally renders PNG previews. `examples/make_fixture.rs`
generates an original EPUB; `examples/verify_import.rs` exercises headless import.
`examples/profile_startup.rs` measures startup and previews the native loading layer.
