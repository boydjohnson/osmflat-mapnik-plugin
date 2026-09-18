#!/bin/bash
# Build a self-contained `render` on macOS: mapnik and every third-party
# library linked in, leaving only macOS' own dylibs/frameworks dynamic (libSystem
# can't be linked statically). Run from the repo root on an arm64 or x86_64 Mac:
#
#   sh scripts/build-static-render-macos.sh
#
# Output: dist/osmflat-render-<version>-<arch>-apple-darwin.tar.gz
set -euo pipefail

BUILD_DIR="${BUILD_DIR:-build-static}"
DIST_DIR="${DIST_DIR:-dist}"
# Binaries built on a newer runner won't start on older macOS unless a
# deployment target is declared. Homebrew's bottles target the runner's OS, so
# this can't go below it.
MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-14.0}"
export MACOSX_DEPLOYMENT_TARGET

# No auto-update / no upgrading of already-installed formulae: this script
# runs on developer machines too, and shouldn't reshuffle someone's Homebrew.
export HOMEBREW_NO_AUTO_UPDATE=1
export HOMEBREW_NO_INSTALLED_DEPENDENTS_CHECK=1
brew install --quiet boost icu4c@76 freetype libpng bzip2 zlib sqlite ninja cmake >/dev/null

# DejaVu is what the styles ask for by face-name, and macOS doesn't ship it.
# Pinned rather than taken from a cask, so both platforms package the same
# font release.
fonts_dir="$BUILD_DIR/dejavu"
if [ ! -d "$fonts_dir" ]; then
    mkdir -p "$fonts_dir"
    curl -fsSL https://github.com/dejavu-fonts/dejavu-fonts/releases/download/version_2_37/dejavu-fonts-ttf-2.37.tar.bz2 \
        | tar xj -C "$fonts_dir" --strip-components=2 'dejavu-fonts-ttf-2.37/ttf/*.ttf'
fi

cmake -S . -B "$BUILD_DIR" -G Ninja \
    -DCMAKE_BUILD_TYPE=Release \
    -DOSMFLAT_STATIC_RENDER=ON \
    -DCMAKE_OSX_DEPLOYMENT_TARGET="$MACOSX_DEPLOYMENT_TARGET"
cmake --build "$BUILD_DIR" --target render --parallel "$(sysctl -n hw.ncpu)"
strip -S -x "$BUILD_DIR/render"

# Nothing outside /usr/lib and /System may be linked, or the binary only runs
# on a machine with Homebrew in the same place.
otool -L "$BUILD_DIR/render"
if otool -L "$BUILD_DIR/render" | tail -n +2 | grep -vE '^\s+(/usr/lib/|/System/)'; then
    echo "error: render links a non-system library (see above)" >&2
    exit 1
fi

MAPNIK_FONT_DIR="$fonts_dir" ctest --test-dir "$BUILD_DIR" --output-on-failure

# uname says arm64; name the archive the way the toolchain triple does.
arch="$(uname -m)"
[ "$arch" = arm64 ] && arch=aarch64
sh scripts/package-render.sh "$BUILD_DIR" "$DIST_DIR" "${arch}-apple-darwin" "$fonts_dir"
