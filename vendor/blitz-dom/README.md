# Vendored Blitz DOM

This directory contains only the `blitz-dom` package from
[DioxusLabs/blitz](https://github.com/DioxusLabs/blitz) at commit
`7931e6794d5d0e791660cc3474536047c01239ed` (version `0.3.0-beta.2`).
Its package manifest is standalone so Cargo can patch this one dependency;
the original `moz-bullet-font.otf` bytes are represented in a Rust array
because this checkout was created with a text-only patch tool.

The local change in `src/layout/inline.rs` preserves the retained Parley layout
around intrinsic (`ComputeSize`) measurement. Line breaks are mutable state
outside Taffy's cached `LayoutOutput`; measuring at another width must not
overwrite the lines used for painting when an ancestor reuses its final layout.
All existing layout caches remain enabled. A temporary layout clone is used
only when an uncached intrinsic measurement needs to run.

`cargo run --example native_preview -- --layout-only` checks exact line ranges,
heading overflow, shelf tabs, and title/action separation through resize round
trips at 100%, 125%, 150%, and 200% scale.

The upstream code and assets retain their MIT OR Apache-2.0 license; see
`LICENSE-MIT` and `LICENSE-APACHE`.
