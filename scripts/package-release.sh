#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
if [[ "$(rustc -vV | sed -n 's/^host: //p')" != x86_64-unknown-linux-gnu ]]; then
    echo 'Package Linux releases on an x86_64-unknown-linux-gnu host.' >&2
    exit 1
fi

version="$(cargo metadata --locked --no-deps --format-version 1 | sed -n 's/.*"name":"mobi-reader","version":"\([^"]*\)".*/\1/p')"
if [[ -z "$version" ]]; then
    echo 'Could not read package version.' >&2
    exit 1
fi

cargo build --locked --release
release_name="mobi-reader-v${version}-linux-x86_64"
stage="dist/$release_name"
if [[ -e "$stage" ]]; then
    echo "Release staging directory already exists: $stage" >&2
    exit 1
fi
mkdir -p "$stage/assets/fonts" "$stage/vendor/blitz-dom"
cp "${CARGO_TARGET_DIR:-target}/release/mobi-reader" README.md LICENSE CHANGELOG.md CONTRIBUTING.md "$stage/"
cp assets/fonts/SourceSerif4-LICENSE.md assets/fonts/SourceSans3-LICENSE.md "$stage/assets/fonts/"
cp vendor/blitz-dom/README.md vendor/blitz-dom/LICENSE-MIT vendor/blitz-dom/LICENSE-APACHE "$stage/vendor/blitz-dom/"
tar -C dist -czf "dist/$release_name.tar.gz" "$release_name"
(cd dist && sha256sum "$release_name.tar.gz" > "$release_name.tar.gz.sha256")
echo "Created dist/$release_name.tar.gz"
