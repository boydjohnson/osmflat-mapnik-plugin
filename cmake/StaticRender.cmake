# OSMFLAT_STATIC_RENDER=ON: build Mapnik from source as a static library with
# the osmflat datasource compiled in as a built-in plugin, and link `render`
# against it. The result has no libmapnik / osmflat.input to find at runtime.
#
# Included from the top-level CMakeLists after the Rust crate is imported
# (the plugin sources need the `osmflat_capi` target and its cbindgen header).

# PROJ (and the dependencies built from source below) have C sources; the
# plugin itself is C++-only, so C is only enabled for this build.
enable_language(C)

set(OSMFLAT_MAPNIK_GIT_TAG "v4.2.2" CACHE STRING "Mapnik tag to build statically")
option(OSMFLAT_FULLY_STATIC "Link render with -static (Linux/musl: no shared libs at all)" OFF)
set(OSMFLAT_HARFBUZZ_GIT_TAG "11.2.1" CACHE STRING "harfbuzz tag built from source on macOS")
set(OSMFLAT_PROJ_GIT_TAG "9.9.0" CACHE STRING "PROJ tag built from source")

if(APPLE AND OSMFLAT_FULLY_STATIC)
    message(FATAL_ERROR "OSMFLAT_FULLY_STATIC needs musl; macOS always links libSystem dynamically")
endif()

# --- Mapnik build options ----------------------------------------------------
# Only what `render` uses: the AGG renderer writing PNG, fonts via FreeType +
# HarfBuzz + ICU, and PROJ (built from source below) for reprojection. No
# Cairo, no stock input plugins.
set(OSMFLAT_MAPNIK_OPTIONS
    BUILD_SHARED_LIBS=OFF
    BUILD_SHARED_PLUGINS=OFF
    BUILD_TESTING=OFF
    BUILD_BENCHMARK=OFF
    BUILD_DEMO_VIEWER=OFF
    BUILD_DEMO_CPP=OFF
    BUILD_UTILITY_GEOMETRY_TO_WKB=OFF
    BUILD_UTILITY_MAPNIK_INDEX=OFF
    BUILD_UTILITY_MAPNIK_RENDER=OFF
    BUILD_UTILITY_OGRINDEX=OFF
    BUILD_UTILITY_PGSQL2SQLITE=OFF
    BUILD_UTILITY_SHAPEINDEX=OFF
    BUILD_UTILITY_SVG2PNG=OFF
    INSTALL_DEPENDENCIES=OFF
    USE_PNG=ON
    USE_JPEG=OFF
    USE_TIFF=OFF
    USE_WEBP=OFF
    USE_AVIF=OFF
    USE_LIBXML2=OFF
    USE_CAIRO=OFF
    USE_PROJ=ON
    USE_GRID_RENDERER=OFF
    USE_SVG_RENDERER=OFF
    # The plugin's MAPNIK_LOG_DEBUG lines (OSMFLAT_LOG_DEBUG=1) need this.
    USE_LOG=ON
    USE_PLUGIN_INPUT_CSV=OFF
    USE_PLUGIN_INPUT_GDAL=OFF
    USE_PLUGIN_INPUT_OGR=OFF
    USE_PLUGIN_INPUT_GDAL_OGR=OFF
    USE_PLUGIN_INPUT_GEOBUF=OFF
    USE_PLUGIN_INPUT_GEOJSON=OFF
    USE_PLUGIN_INPUT_POSTGIS=OFF
    USE_PLUGIN_INPUT_PGRASTER=OFF
    USE_PLUGIN_INPUT_POSTGIS_PGRASTER=OFF
    USE_PLUGIN_INPUT_RASTER=OFF
    USE_PLUGIN_INPUT_SHAPE=OFF
    USE_PLUGIN_INPUT_SQLITE=OFF
    USE_PLUGIN_INPUT_TILES=OFF
    USE_PLUGIN_INPUT_TOPOJSON=OFF
)
foreach(opt IN LISTS OSMFLAT_MAPNIK_OPTIONS)
    string(REPLACE "=" ";" kv "${opt}")
    list(GET kv 0 key)
    list(GET kv 1 value)
    set(${key} ${value} CACHE BOOL "" FORCE)
endforeach()

# Static dependency archives (libicuuc.a, libfreetype.a, ...) rather than .so.
set(Boost_USE_STATIC_LIBS ON)
set(ICU_USE_STATIC_LIBS ON)

if(APPLE)
    # Everything third-party goes in; only macOS' own dylibs stay dynamic.
    set(CMAKE_FIND_LIBRARY_SUFFIXES .a)
    # Homebrew's Boost::regex names icu libs as bare `icuuc`/`icudata`/... which
    # the linker can't resolve (icu4c is keg-only) and which would anyway pick
    # the dylib over the archive. Mapnik's own workaround drops them; it links
    # ICU::uc/data/i18n itself, by full path to the .a.
    set(USE_BOOST_REGEX_ICU_WORKAROUND ON CACHE BOOL "" FORCE)
    # Homebrew keeps icu4c, zlib, bzip2 and sqlite keg-only: their archives
    # aren't symlinked into the prefix, so nothing finds libicuuc.a / libz.a /
    # libbz2.a / libsqlite3.a without being told where they live.
    find_program(OSMFLAT_BREW brew)
    if(OSMFLAT_BREW)
        if(NOT ICU_ROOT AND NOT DEFINED ENV{ICU_ROOT})
            # Homebrew versions icu4c; take whichever of these is installed.
            foreach(icu_formula icu4c@76 icu4c@77 icu4c)
                if(NOT ICU_ROOT)
                    execute_process(COMMAND ${OSMFLAT_BREW} --prefix ${icu_formula}
                        OUTPUT_VARIABLE ICU_ROOT OUTPUT_STRIP_TRAILING_WHITESPACE
                        ERROR_QUIET)
                endif()
            endforeach()
        endif()
        foreach(keg zlib bzip2 sqlite)
            execute_process(COMMAND ${OSMFLAT_BREW} --prefix ${keg}
                OUTPUT_VARIABLE keg_prefix OUTPUT_STRIP_TRAILING_WHITESPACE
                ERROR_QUIET)
            if(keg_prefix)
                list(APPEND CMAKE_PREFIX_PATH "${keg_prefix}")
            endif()
        endforeach()
    endif()

    # Homebrew's libharfbuzz.a is built against glib and graphite2, and
    # graphite2 has no static archive -- so a binary using it would still need
    # Homebrew at runtime. Build harfbuzz ourselves instead, with just freetype
    # and CoreText, which drops glib/graphite2/pcre2/intl entirely.
    set(HB_HAVE_FREETYPE ON CACHE BOOL "" FORCE)
    set(HB_HAVE_CORETEXT ON CACHE BOOL "" FORCE)
    set(HB_HAVE_GLIB OFF CACHE BOOL "" FORCE)
    set(HB_HAVE_GRAPHITE2 OFF CACHE BOOL "" FORCE)
    set(HB_HAVE_ICU OFF CACHE BOOL "" FORCE)
    set(HB_BUILD_SUBSET OFF CACHE BOOL "" FORCE)
    set(HB_BUILD_UTILS OFF CACHE BOOL "" FORCE)
    set(HB_BUILD_TESTS OFF CACHE BOOL "" FORCE)
    FetchContent_Declare(
        harfbuzz
        GIT_REPOSITORY https://github.com/harfbuzz/harfbuzz.git
        GIT_TAG ${OSMFLAT_HARFBUZZ_GIT_TAG}
        GIT_SHALLOW TRUE
        EXCLUDE_FROM_ALL
    )
    FetchContent_MakeAvailable(harfbuzz)

    # Mapnik looks for harfbuzz with find_package(CONFIG) and expects a
    # harfbuzz::harfbuzz target. Point that lookup at a generated config that
    # aliases the target we just built, so it never sees Homebrew's (which
    # would also collide on the target name).
    set(shim "${CMAKE_BINARY_DIR}/harfbuzz-config-shim")
    file(WRITE "${shim}/harfbuzzConfig.cmake"
"set(harfbuzz_FOUND TRUE)
set(harfbuzz_VERSION \"${OSMFLAT_HARFBUZZ_GIT_TAG}\")
if(NOT TARGET harfbuzz::harfbuzz)
    add_library(harfbuzz::harfbuzz ALIAS harfbuzz)
endif()
")
    set(harfbuzz_DIR "${shim}" CACHE PATH "" FORCE)
endif()
if(OSMFLAT_FULLY_STATIC)
    set(CMAKE_FIND_LIBRARY_SUFFIXES .a)
    # --static so pkg-config also reports Libs.private (harfbuzz needs glib,
    # graphite2, freetype's bz2/brotli, ...), which only matters when linking
    # against the archives.
    set(PKG_CONFIG_ARGN --static)
    # harfbuzz's own CMake config names the shared library, which a -static
    # link refuses ("attempted static link of dynamic object"). Hiding the
    # config package makes Mapnik take its pkg-config fallback instead.
    set(CMAKE_DISABLE_FIND_PACKAGE_harfbuzz ON)
    # pkg_check_modules caches harfbuzz_FOUND; left over from an earlier
    # configure it would send Mapnik down the (now disabled) config branch and
    # fail on a missing harfbuzz::harfbuzz target.
    unset(harfbuzz_FOUND CACHE)
endif()

# --- PROJ ----------------------------------------------------------------------
# Built from source on every platform: both Alpine's and Homebrew's PROJ are
# built with libtiff + libcurl (for reading/downloading datum-shift grids),
# which would drag OpenSSL and friends into the binary. Without them, a
# transformation that needs a grid falls back to PROJ's ballpark one --
# roughly a metre between WGS84 and NAD83, invisible at map scale.
#
# A static PROJ embeds proj.db in the library (EMBED_RESOURCE_FILES defaults
# ON when BUILD_SHARED_LIBS is OFF); USE_ONLY_EMBEDDED_RESOURCE_FILES stops it
# also looking for a proj.db on disk, so the binary behaves the same
# everywhere and there's no data directory to ship.
foreach(opt
        ENABLE_TIFF=OFF ENABLE_CURL=OFF BUILD_PROJSYNC=OFF
        BUILD_APPS=OFF BUILD_TESTING=OFF
        EMBED_RESOURCE_FILES=ON USE_ONLY_EMBEDDED_RESOURCE_FILES=ON)
    string(REPLACE "=" ";" kv "${opt}")
    list(GET kv 0 key)
    list(GET kv 1 value)
    set(${key} ${value} CACHE BOOL "" FORCE)
endforeach()
set(NLOHMANN_JSON_ORIGIN "internal" CACHE STRING "" FORCE)
FetchContent_Declare(
    proj
    GIT_REPOSITORY https://github.com/OSGeo/PROJ.git
    GIT_TAG ${OSMFLAT_PROJ_GIT_TAG}
    GIT_SHALLOW TRUE
    EXCLUDE_FROM_ALL
)
FetchContent_MakeAvailable(proj)

# Mapnik looks for PROJ with find_package(PROJ) and reads PROJ_LIBRARIES /
# PROJ_INCLUDE_DIRS / PROJ_VERSION_*. Point that at a generated config for the
# target we just built, as with harfbuzz on macOS. PROJ_INCLUDE_DIRS must be a
# single path: mapnik wraps it in $<BUILD_INTERFACE:...>, which a ;-list would
# split, leaking a bare source-tree path into its install(EXPORT). The `proj`
# target carries the rest of its include dirs itself.
string(REPLACE "." ";" _proj_v "${OSMFLAT_PROJ_GIT_TAG}")
list(GET _proj_v 0 _proj_major)
list(GET _proj_v 1 _proj_minor)
list(GET _proj_v 2 _proj_patch)
set(_proj_shim "${CMAKE_BINARY_DIR}/proj-config-shim")
file(WRITE "${_proj_shim}/PROJConfig.cmake"
"set(PROJ_FOUND TRUE)
set(PROJ_VERSION \"${OSMFLAT_PROJ_GIT_TAG}\")
set(PROJ_VERSION_MAJOR ${_proj_major})
set(PROJ_VERSION_MINOR ${_proj_minor})
set(PROJ_VERSION_PATCH ${_proj_patch})
set(PROJ_LIBRARIES proj)
set(PROJ_INCLUDE_DIRS \"${proj_SOURCE_DIR}/src\")
")
set(PROJ_DIR "${_proj_shim}" CACHE PATH "" FORCE)

# Read by the generated plugins/input/osmflat/CMakeLists.txt.
set(OSMFLAT_SOURCE_DIR ${CMAKE_SOURCE_DIR})

FetchContent_Declare(
    mapnik
    GIT_REPOSITORY https://github.com/mapnik/mapnik.git
    GIT_TAG ${OSMFLAT_MAPNIK_GIT_TAG}
    GIT_SHALLOW TRUE
    GIT_SUBMODULES deps/mapbox/geometry deps/mapbox/polylabel deps/mapbox/protozero deps/mapbox/variant
    PATCH_COMMAND ${CMAKE_COMMAND}
        -DMAPNIK_SOURCE_DIR=<SOURCE_DIR>
        -DOSMFLAT_SOURCE_DIR=${CMAKE_SOURCE_DIR}
        -P ${CMAKE_SOURCE_DIR}/cmake/static-mapnik/patch-mapnik.cmake
    UPDATE_DISCONNECTED TRUE
    EXCLUDE_FROM_ALL
)
FetchContent_MakeAvailable(mapnik)

# libmapnik now compiles our plugin sources, which include the cbindgen header
# that `cargo build` writes -- so cargo has to run first.
add_dependencies(mapnik cargo-build_osmflat_capi)

# Cargo reports `gcc_s` among the native libs its staticlib needs, but musl
# toolchains ship only the shared libgcc_s -- the static unwinder there is
# libgcc_eh.a. Swap it in Corrosion's link interface, or `-static` fails with
# "cannot find -lgcc_s".
if(OSMFLAT_FULLY_STATIC)
    foreach(tgt osmflat_capi osmflat_capi-static)
        if(TARGET ${tgt})
            get_target_property(_libs ${tgt} INTERFACE_LINK_LIBRARIES)
            if(_libs)
                string(REPLACE "gcc_s" "gcc_eh" _libs "${_libs}")
                set_target_properties(${tgt} PROPERTIES INTERFACE_LINK_LIBRARIES "${_libs}")
            endif()
        endif()
    endforeach()
endif()

# --- render ------------------------------------------------------------------
# ICU's data (break iterators for label wrapping, the tables Boost.Regex's ICU
# traits need for any .match()/.replace() filter) normally lives in
# libicudata.a. Alpine builds ICU with archive packaging, so its libicudata.a
# is a ~1 KB stub and the data is a separate icudt<ver>l.dat that a relocated
# binary can't find -- regex filters then throw "Could not initialize ICU
# resources" and wrapping logs "could not create BreakIterator". Pointing
# OSMFLAT_ICU_DATA_FILE at that .dat embeds it; it has to be the same ICU
# version the binary links, which it is when both come from one distro.
set(OSMFLAT_ICU_DATA_FILE "" CACHE FILEPATH
    "ICU common data (.dat) to embed in render, for stub-libicudata builds (Alpine)")
if(OSMFLAT_ICU_DATA_FILE)
    if(APPLE)
        message(FATAL_ERROR "OSMFLAT_ICU_DATA_FILE is ELF-only; Homebrew's libicudata.a already carries the data")
    endif()
    if(NOT EXISTS "${OSMFLAT_ICU_DATA_FILE}")
        message(FATAL_ERROR "OSMFLAT_ICU_DATA_FILE does not exist: ${OSMFLAT_ICU_DATA_FILE}")
    endif()
    enable_language(ASM)
    configure_file(${CMAKE_SOURCE_DIR}/cmake/icu-data.S.in ${CMAKE_BINARY_DIR}/icu-data.S @ONLY)
    # .incbin isn't a dependency CMake can see; rebuild when the .dat changes.
    set_source_files_properties(${CMAKE_BINARY_DIR}/icu-data.S PROPERTIES
        OBJECT_DEPENDS "${OSMFLAT_ICU_DATA_FILE}")
endif()

add_executable(render test/render.cpp)
if(OSMFLAT_ICU_DATA_FILE)
    target_sources(render PRIVATE ${CMAKE_BINARY_DIR}/icu-data.S)
    target_compile_definitions(render PRIVATE OSMFLAT_EMBEDDED_ICU_DATA)
endif()
# mapnik::mapnik before osmflat_capi: libmapnik.a references the Rust symbols,
# and single-pass linkers resolve left to right.
target_link_libraries(render PRIVATE mapnik::mapnik osmflat_capi)
if(APPLE)
    # Homebrew's libfreetype.a carries its bzip2/brotli font decompressors, but
    # FindFreetype reports only the archive itself.
    find_library(OSMFLAT_BZ2_LIBRARY NAMES bz2)
    find_library(OSMFLAT_BROTLIDEC_LIBRARY NAMES brotlidec)
    find_library(OSMFLAT_BROTLICOMMON_LIBRARY NAMES brotlicommon)
    foreach(lib OSMFLAT_BZ2_LIBRARY OSMFLAT_BROTLIDEC_LIBRARY OSMFLAT_BROTLICOMMON_LIBRARY)
        if(${lib})
            target_link_libraries(render PRIVATE ${${lib}})
        endif()
    endforeach()
endif()
target_compile_definitions(render PRIVATE OSMFLAT_STATIC_RENDER)
if(OSMFLAT_FULLY_STATIC)
    target_link_options(render PRIVATE -static)
endif()

enable_testing()
set(FIXTURES ${CMAKE_SOURCE_DIR}/test/fixtures)
# Same smoke test as the dynamic build. The plugin_dir argument is ignored by
# a static render, so pass a path that doesn't exist to prove it.
add_test(NAME render_relations
    COMMAND render ${CMAKE_BINARY_DIR}/no-plugins ${CMAKE_SOURCE_DIR}/test/style-relations.xml
        ${FIXTURES}/baarle-hertog.osm.flat ${CMAKE_BINARY_DIR}/render_relations.png
        4.75 51.38 5.02 51.49 500 500)

# The same fixture drawn in the Dutch national grid (EPSG:28992, RD New, on
# the Bessel ellipsoid): exercises PROJ end to end -- the EPSG lookup has to
# come from the proj.db embedded in the binary, since the smoke test runs where
# no proj.db exists. The bbox is the render_relations one, transformed.
# ICU data, not just code: a regex filter plus a wrapped label (see
# test/style-icu.xml). ICU_DATA points at nothing so a distro's data dir in the
# build environment can't mask a binary that lacks its own data -- which is
# exactly how this slipped through on Alpine before.
set(_icu_png ${CMAKE_BINARY_DIR}/render_icu_data.png)
add_test(NAME render_icu_data
    COMMAND ${CMAKE_COMMAND}
        -DRENDER=$<TARGET_FILE:render> -DOUT=${_icu_png} -DMIN_BYTES=4000
        "-DFORBID=BreakIterator|ICU resources|U_MISSING_RESOURCE_ERROR"
        "-DARGS=${CMAKE_BINARY_DIR}/no-plugins;${CMAKE_SOURCE_DIR}/test/style-icu.xml;${FIXTURES}/baarle-hertog.osm.flat;${_icu_png};4.75;51.38;5.02;51.49;500;500"
        -P ${CMAKE_SOURCE_DIR}/cmake/check-render.cmake)
set_tests_properties(render_icu_data PROPERTIES
    ENVIRONMENT "ICU_DATA=${CMAKE_BINARY_DIR}/no-icu-data")

set(_rd_png ${CMAKE_BINARY_DIR}/render_relations_rd.png)
add_test(NAME render_relations_projected
    COMMAND ${CMAKE_COMMAND}
        -DRENDER=$<TARGET_FILE:render> -DOUT=${_rd_png} -DMIN_BYTES=8000
        "-DARGS=${CMAKE_BINARY_DIR}/no-plugins;${CMAKE_SOURCE_DIR}/test/style-relations-rd.xml;${FIXTURES}/baarle-hertog.osm.flat;${_rd_png};110639;376952;129497;389060;600;385"
        -P ${CMAKE_SOURCE_DIR}/cmake/check-render.cmake)
