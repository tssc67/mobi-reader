# Mobi Reader

<p align="center"><img src="assets/icon.svg" alt="Mobi Reader app icon" width="128" height="128"></p>

A quiet, native Windows and Linux reader for your own EPUB and DRM-free legacy MOBI books.
Built in Rust with Dioxus Native, Blitz, and Vello/wgpu. No browser server,
WebView2, or Calibre installation is required.

## Download

Get the Windows x86-64 ZIP from
[Releases](https://github.com/tssc67/mobi-reader/releases/latest), or build the
Linux x86-64 tarball from source with the instructions below. Run
`mobi-reader.exe` on Windows or `./mobi-reader` on Linux. A working
wgpu-compatible graphics driver is required. Binaries are currently unsigned;
Windows may show a reputation warning. Keep the included license notices with
redistributed copies.

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

Install Rust **1.96 or newer**. On Windows, install Visual Studio C++ Build Tools
and the Windows SDK, and use the `x86_64-pc-windows-msvc` toolchain.

```powershell
git clone https://github.com/tssc67/mobi-reader.git
cd mobi-reader
cargo run --locked
cargo build --locked --release
.\target\release\mobi-reader.exe
```

Styles, fonts, and icons are embedded in the executable. Keep Cargo.lock committed;
the native renderer is pinned and carries a small documented local layout fix.

On Ubuntu 24.04 or WSL, install the native build and runtime libraries, then build
with the Linux Rust toolchain:

```sh
sudo apt-get update
sudo apt-get install build-essential pkg-config libx11-dev libxcursor-dev libxi-dev libxrandr-dev libxkbcommon-dev libxkbcommon-x11-0 libwayland-dev libasound2-dev libudev-dev libssl-dev libfontconfig1-dev libfreetype6-dev libgtk-3-dev libdbus-1-dev
cargo test --locked --all-targets
cargo build --locked --release
./target/release/mobi-reader
```

WSLg and other Linux desktops use their normal Wayland or X11 display selection.
If WSLg's Weston compositor disconnects, X11 remains available as a fallback:
`env -u WAYLAND_DISPLAY ./target/release/mobi-reader`. Linux builds can be
packaged with `bash scripts/package-release.sh`.

## Documentation

- [Website](https://tssc67.github.io/mobi-reader/) and [publishing / search indexing](docs/website.md).
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
