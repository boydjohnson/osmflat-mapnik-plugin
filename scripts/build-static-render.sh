#!/bin/sh
# Build a fully static `render` (musl, Mapnik + osmflat compiled in) inside an
# Alpine container, run the smoke test, and package it with DejaVu fonts.
#
# Run from the repo root, in CI or locally:
#
#   docker run --rm -v "$PWD:/src" -w /src alpine:3.22 sh scripts/build-static-render.sh
#   podman run --rm -v "$PWD:/src" -w /src alpine:3.22 sh scripts/build-static-render.sh
#
# Output: dist/osmflat-render-<version>-<arch>-linux-musl.tar.gz
# (packaging itself lives in scripts/package-render.sh, shared with macOS)
#
# Everything reusable lives under $BUILD_DIR (mapnik's fetched source + objects,
# the cargo registry, downloaded apk packages) so CI can cache that one path and
# a second run only relinks what changed.
set -eu

BUILD_DIR="${BUILD_DIR:-build-static}"
DIST_DIR="${DIST_DIR:-dist}"
# Absolute: cmake/cargo are invoked from various working directories.
CARGO_HOME="${CARGO_HOME:-$PWD/$BUILD_DIR/cargo-home}"
APK_CACHE="${APK_CACHE:-$PWD/$BUILD_DIR/apk-cache}"
export CARGO_HOME
mkdir -p "$APK_CACHE"

apk add --cache-dir "$APK_CACHE" \
    build-base cmake samurai git pkgconf curl ca-certificates linux-headers file \
    rust cargo \
    boost1.84-dev boost1.84-static \
    icu-dev icu-static \
    freetype-dev freetype-static \
    harfbuzz-dev harfbuzz-static graphite2-static glib-static pcre2-static \
    libpng-dev libpng-static zlib-dev zlib-static bzip2-static brotli-static expat-static \
    font-dejavu

cmake -S . -B "$BUILD_DIR" -G Ninja \
    -DCMAKE_BUILD_TYPE=Release \
    -DOSMFLAT_STATIC_RENDER=ON \
    -DOSMFLAT_FULLY_STATIC=ON
cmake --build "$BUILD_DIR" --target render --parallel "$(nproc)"
strip "$BUILD_DIR/render"

file "$BUILD_DIR/render"
if file "$BUILD_DIR/render" | grep -q 'dynamically linked'; then
    echo "error: render is dynamically linked" >&2
    exit 1
fi

MAPNIK_FONT_DIR=/usr/share/fonts/dejavu ctest --test-dir "$BUILD_DIR" --output-on-failure

sh scripts/package-render.sh "$BUILD_DIR" "$DIST_DIR" "$(uname -m)-linux-musl" /usr/share/fonts/dejavu
