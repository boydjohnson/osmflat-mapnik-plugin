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
set -eu

BUILD_DIR="${BUILD_DIR:-build-static}"
DIST_DIR="${DIST_DIR:-dist}"

apk add --no-cache \
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

version="$(sed -n 's/^project(osmflat-mapnik-plugin VERSION \([^ )]*\).*/\1/p' CMakeLists.txt)"
name="osmflat-render-${version}-$(uname -m)-linux-musl"
rm -rf "$DIST_DIR/$name"
mkdir -p "$DIST_DIR/$name/fonts"
cp "$BUILD_DIR/render" "$DIST_DIR/$name/"
cp /usr/share/fonts/dejavu/*.ttf "$DIST_DIR/$name/fonts/"
tar -C "$DIST_DIR" -czf "$DIST_DIR/$name.tar.gz" "$name"
ls -lh "$DIST_DIR/$name/render" "$DIST_DIR/$name.tar.gz"
