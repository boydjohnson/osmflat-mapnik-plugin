# OSMFLAT_STATIC_RENDER=ON: build Mapnik from source as a static library with
# the osmflat datasource compiled in as a built-in plugin, and link `render`
# against it. The result has no libmapnik / osmflat.input to find at runtime.
#
# Included from the top-level CMakeLists after the Rust crate is imported
# (the plugin sources need the `osmflat_capi` target and its cbindgen header).

set(OSMFLAT_MAPNIK_GIT_TAG "v4.2.2" CACHE STRING "Mapnik tag to build statically")
option(OSMFLAT_FULLY_STATIC "Link render with -static (Linux/musl: no shared libs at all)" OFF)

# --- Mapnik build options ----------------------------------------------------
# Only what `render` uses: the AGG renderer writing PNG, fonts via FreeType +
# HarfBuzz + ICU. No PROJ (render rewrites the WGS84 / web-mercator proj4
# strings to their EPSG codes, which Mapnik handles natively), no Cairo, no
# stock input plugins.
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
    USE_PROJ=OFF
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
add_executable(render test/render.cpp)
# mapnik::mapnik before osmflat_capi: libmapnik.a references the Rust symbols,
# and single-pass linkers resolve left to right.
target_link_libraries(render PRIVATE mapnik::mapnik osmflat_capi)
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
