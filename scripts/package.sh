#!/bin/sh
# Packages a release build into dist/.
#
# Usage: scripts/package.sh <rust-target> <platform> <version>
#   scripts/package.sh x86_64-unknown-linux-gnu x86_64-linux 0.1.0
#
# Produces, in dist/:
#   Linux    <app>-<platform>.tar.xz  binary, desktop entry and icons
# Build the release binary for <rust-target> first. This app builds for
# Linux only.
set -eu
cd "$(dirname "$0")/.."

target=$1
platform=$2
version=$3
app=$(sed -n 's/^name = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
bin=target/$target/release/$app
name=$app-$platform
stage=dist/$name

rm -rf "$stage"
mkdir -p "$stage"

case $target in
*-linux-*)
    cp LICENSE README.md "$bin" "$stage/"
    mkdir -p "$stage/share/applications"
    cp packaging/linux/app.desktop "$stage/share/applications/$app.desktop"
    # A vector icon ships as one SVG; pixel art ships as the rasters only,
    # since an SVG of it would just be the same pixels scaled up.
    if [ -f assets/icon.svg ]; then
        mkdir -p "$stage/share/icons/hicolor/scalable/apps"
        cp assets/icon.svg "$stage/share/icons/hicolor/scalable/apps/$app.svg"
    fi
    for size in 16 24 32 48 64 128 256 512; do
        dir=$stage/share/icons/hicolor/${size}x$size/apps
        mkdir -p "$dir"
        cp "assets/icons/$size.png" "$dir/$app.png"
    done
    tar -C dist -cJf "$stage.tar.xz" "$name"
    ;;
*)
    echo "unsupported target: $target (Linux only)" >&2
    exit 1
    ;;
esac

rm -rf "$stage"
ls dist/"$name".*
