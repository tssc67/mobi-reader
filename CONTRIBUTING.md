# Contributing

Feel free to contribute! Bug reports, documentation improvements, accessibility
work, and focused pull requests are welcome.

For a larger feature, open an issue first so we can agree on the scope. For bugs,
include the operating system and version, display server, display scale, graphics adapter, application version,
reproduction steps, and expected versus actual behavior. Do not upload private or
copyrighted books; use a small original fixture or link to a legally available sample.

## Development

Install Rust 1.96 or newer. On Windows, install the Visual Studio C++ build tools
with the Windows SDK. On Linux, install the native libraries listed in the README.
Clone the repository and run `cargo run --locked`. No Dioxus CLI is needed.
Set `MOBI_READER_DATA_DIR` to a disposable directory to isolate development data.

Before opening a pull request, run:

```powershell
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo run --locked --example native_preview -- --layout-only
```

For UI changes, also run `cargo run --locked --example native_preview`, inspect
the images in `target/previews`, and check the real window at multiple display
scales. Offscreen checks do not cover dialogs, keyboard focus, or window events.

Keep changes focused and describe the problem, resulting behavior, and validation
in the pull request. Add regression coverage for behavioral fixes. Keep Cargo.lock
committed; do not include build output, personal books, or library databases.

## Licensing and dependencies

Contributions to application code are provided under the project's MIT license.
Only contribute work you have permission to license. The MOBI converter is an
independent implementation; do not copy code from Calibre or other GPL projects.
Preserve third-party notices. Changes to the vendored renderer must be documented
in `vendor/blitz-dom/README.md`, with the upstream revision and local differences.

See [architecture](docs/architecture.md) and [release instructions](docs/releases.md).
