# Releases

Published releases currently target Windows x86-64. Linux x86-64 release archives
can be built with `bash scripts/package-release.sh` on a Linux host. A working
wgpu-compatible graphics driver is required. The executable needs no adjacent
application assets. Distribute the license notices with it. Binaries are unsigned.

## Maintainer checklist

1. Update the package version in Cargo.toml and its Cargo.lock entry; add release
   notes to CHANGELOG.md.
2. Run the checks in CONTRIBUTING.md and inspect native previews. Manually check
   import, reading, reopening, keyboard navigation, and window resizing.
3. Run `./scripts/package-release.ps1` on Windows and
   `bash scripts/package-release.sh` on Linux. These build with the committed
   lockfile and create versioned archives with SHA-256 checksums under `dist`.
4. Commit the source and create an annotated `vX.Y.Z` tag on that commit.
5. Push the commit and tag, then publish with `gh release create --verify-tag`,
   attaching the archives and checksums. Use `--notes-file` for release notes.
6. Check the public release page and uploaded asset names/sizes.

Do not publish `target`, personal library data, test books, or credentials. Never
replace an existing release tag silently. CI verifies source changes; publishing
a release is an explicit maintainer operation.

The archives contain the executable, README, changelog, MIT license, font notices, and
the vendored renderer's MIT/Apache notices. Other dependencies retain their own
licenses, identified by Cargo.lock and their upstream packages.
