# Mobi Reader

A quiet, native Windows reader for your own EPUB and DRM-free legacy MOBI books.
Built in Rust with Dioxus Native, Blitz, and Vello/wgpu. No browser server,
WebView2, or Calibre installation is required.

## Download

Get the Windows x86-64 ZIP from [Releases](https://github.com/tssc67/mobi-reader/releases/latest).
Extract it and run `mobi-reader.exe`. Windows 10/11 with a working GPU driver is
the intended platform. Binaries are currently unsigned; Windows may show a
reputation warning. Keep the included license notices with redistributed copies.

## Features

- Local library with covers, search, sorting, and reading/finished shelves.
- EPUB 2/3 and built-in legacy MOBI conversion, preserving original files.
- Scrollable chapters, table of contents, book search, bookmarks, and saved position.
- Paper, sepia, and night themes with adjustable type size, spacing, and width.
- File picker, drag-and-drop, and command-line imports; no cloud account needed.

The reader normalizes book styling for a consistent reading experience. DRM,
pure KF8, fixed-layout EPUB, PDF, and AZW/KFX are not supported. It does not offer
cloud sync, annotations, or paginated spreads yet.

## Build

Install Rust **1.96 or newer**, Visual Studio C++ Build Tools, and the Windows SDK.
Use the `x86_64-pc-windows-msvc` toolchain.

```powershell
git clone https://github.com/tssc67/mobi-reader.git
cd mobi-reader
cargo run --locked
cargo build --locked --release
.\target\release\mobi-reader.exe
```

Styles, fonts, and icons are embedded in the executable. Keep Cargo.lock committed;
the native renderer is pinned and carries a small documented local layout fix.

## Documentation

- [User guide](docs/user-guide.md): importing, formats, local storage, limits, and diagnostics.
- [Architecture](docs/architecture.md): source map, data flow, and the renderer patch.
- [Contributing](CONTRIBUTING.md): setup, checks, and pull request guidance.
- [Releases](docs/releases.md): packaging and publishing.
- [Changelog](CHANGELOG.md): version history.

## Contributing

**Feel free to contribute!** Issues and focused pull requests are welcome, including
bug reports, accessibility improvements, documentation, and format compatibility.
Please read [CONTRIBUTING.md](CONTRIBUTING.md) before starting a larger change.

## License

Application code is [MIT licensed](LICENSE). The MOBI converter is independently
implemented; no Calibre code was copied or adapted. Adobe Source fonts retain
their [SIL Open Font License notices](assets/fonts). The vendored Blitz component
is MIT OR Apache-2.0; see [its provenance and licenses](vendor/blitz-dom/README.md).
Other dependencies retain their respective licenses.
