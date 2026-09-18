#!/bin/sh
# Package a built `render` into dist/osmflat-render-<version>-<platform>.tar.gz
# with the fonts it falls back to and the licenses it has to carry.
#
#   package-render.sh <build-dir> <dist-dir> <platform-tag> <fonts-dir>
#
# A tag build passes OSMFLAT_RENDER_VERSION so the archive name matches the
# release; otherwise the project version is used.
set -eu

BUILD_DIR="$1"
DIST_DIR="$2"
PLATFORM="$3"
FONTS_DIR="$4"

version="${OSMFLAT_RENDER_VERSION:-$(sed -n 's/^project(osmflat-mapnik-plugin VERSION \([^ )]*\).*/\1/p' CMakeLists.txt)}"
name="osmflat-render-${version}-${PLATFORM}"
rm -rf "$DIST_DIR/$name"
mkdir -p "$DIST_DIR/$name/fonts"
cp "$BUILD_DIR/render" "$DIST_DIR/$name/"
cp "$FONTS_DIR"/*.ttf "$DIST_DIR/$name/fonts/"
cp LICENSE-MIT README.md "$DIST_DIR/$name/"

# mapnik is LGPL-2.1 and linked into the binary, so ship its license text and
# say where the sources and relink instructions are.
cp "$BUILD_DIR/_deps/mapnik-src/COPYING" "$DIST_DIR/$name/LICENSE.mapnik"
mapnik_tag="$(git -C "$BUILD_DIR/_deps/mapnik-src" describe --tags --always 2>/dev/null || echo unknown)"
cat > "$DIST_DIR/$name/NOTICE" <<NOTICE
osmflat-render ${version} -- static build (${PLATFORM})

osmflat-mapnik-plugin itself is MIT licensed (LICENSE-MIT).

This binary statically links mapnik (${mapnik_tag}), which is licensed under
the GNU Lesser General Public License v2.1 (see LICENSE.mapnik). To exercise
your right to relink it against a modified mapnik, build from source:

  https://github.com/boydjohnson/osmflat-mapnik-plugin
  cmake -S . -B build-static -DCMAKE_BUILD_TYPE=Release -DOSMFLAT_STATIC_RENDER=ON
  cmake --build build-static --target render

scripts/build-static-render.sh (linux/musl) and
scripts/build-static-render-macos.sh reproduce these archives, and
cmake/static-mapnik/ holds the patches applied to the mapnik source tree.

Also linked in: boost (BSL-1.0), ICU (Unicode-3.0), freetype (FTL), harfbuzz
(MIT), PROJ (MIT, with its proj.db embedded), SQLite (public domain),
nlohmann/json (MIT), libpng (PNG-2.0), zlib (Zlib), brotli (MIT),
bzip2 (bzip2-1.0.6).
The linux/musl build additionally links glib (LGPL-2.1), graphite2
(LGPL-2.1), pcre2 (BSD-3-Clause) and musl (MIT).

fonts/ contains the DejaVu fonts (Bitstream Vera / public-domain derived):
https://dejavu-fonts.github.io/License.html
NOTICE

tar -C "$DIST_DIR" -czf "$DIST_DIR/$name.tar.gz" "$name"
ls -lh "$DIST_DIR/$name/render" "$DIST_DIR/$name.tar.gz"
