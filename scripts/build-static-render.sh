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

# A tag build passes the tag's version so the archive name matches the release;
# otherwise take the project version.
version="${OSMFLAT_RENDER_VERSION:-$(sed -n 's/^project(osmflat-mapnik-plugin VERSION \([^ )]*\).*/\1/p' CMakeLists.txt)}"
name="osmflat-render-${version}-$(uname -m)-linux-musl"
rm -rf "$DIST_DIR/$name"
mkdir -p "$DIST_DIR/$name/fonts"
cp "$BUILD_DIR/render" "$DIST_DIR/$name/"
cp /usr/share/fonts/dejavu/*.ttf "$DIST_DIR/$name/fonts/"
cp LICENSE-MIT README.md "$DIST_DIR/$name/"

# mapnik is LGPL-2.1 and linked into the binary, so ship its license text and
# say where the sources and relink instructions are.
cp "$BUILD_DIR/_deps/mapnik-src/COPYING" "$DIST_DIR/$name/LICENSE.mapnik"
mapnik_tag="$(git -C "$BUILD_DIR/_deps/mapnik-src" describe --tags --always 2>/dev/null || echo unknown)"
cat > "$DIST_DIR/$name/NOTICE" <<NOTICE
osmflat-render ${version} -- static build

osmflat-mapnik-plugin itself is MIT licensed (LICENSE-MIT).

This binary statically links mapnik (${mapnik_tag}), which is licensed under
the GNU Lesser General Public License v2.1 (see LICENSE.mapnik). To exercise
your right to relink it against a modified mapnik, build from source:

  https://github.com/boydjohnson/osmflat-mapnik-plugin
  cmake -S . -B build-static -DCMAKE_BUILD_TYPE=Release \\
      -DOSMFLAT_STATIC_RENDER=ON -DOSMFLAT_FULLY_STATIC=ON
  cmake --build build-static --target render

scripts/build-static-render.sh reproduces this exact archive in an alpine
container, and cmake/static-mapnik/ holds the patches applied to the mapnik
source tree.

Also linked in: boost (BSL-1.0), ICU (Unicode-3.0), freetype (FTL), harfbuzz
(MIT), libpng (PNG-2.0), zlib (Zlib), glib (LGPL-2.1), graphite2 (LGPL-2.1),
brotli (MIT), bzip2 (bzip2-1.0.6), pcre2 (BSD-3-Clause), musl (MIT).

fonts/ contains the DejaVu fonts (Bitstream Vera / public-domain derived):
https://dejavu-fonts.github.io/License.html
NOTICE

tar -C "$DIST_DIR" -czf "$DIST_DIR/$name.tar.gz" "$name"
ls -lh "$DIST_DIR/$name/render" "$DIST_DIR/$name.tar.gz"
