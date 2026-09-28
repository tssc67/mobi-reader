# Changelog

## Unreleased

- Added Linux source builds, Linux CI, and an x86-64 release packaging script.
- Verified WSLg Wayland launch and EPUB import; documented X11 as a fallback if
  the WSLg compositor disconnects.

## 0.1.0 - 2026-09-28

Initial public Windows release.

- Native Dioxus/Blitz interface rendered with Vello and wgpu.
- Local EPUB library and independent built-in legacy MOBI conversion.
- Search, contents, bookmarks, reading progress, and appearance preferences.
- Embedded fonts and icon, with a loading animation inside the main window.
- Stable title wrapping during resizing and a responsive continue-reading banner.

Known limits: DRM, pure KF8, fixed-layout EPUB, PDF, AZW/KFX, cloud sync and
annotations are unsupported. Publisher styling is normalized. See the user guide
for supported formats, storage behavior and import limits. Windows binaries are
unsigned. Automated checks do not cover every native GUI interaction.
