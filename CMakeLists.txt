cmake_minimum_required(VERSION 3.22)
project(osmflat-mapnik-plugin VERSION 0.1.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

# Emit compile_commands.json so clangd/IDEs resolve mapnik & generated headers.
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

# --- Rust crate via Corrosion ------------------------------------------------
# Corrosion drives `cargo build` for the staticlib as part of `cmake --build`,
# so the whole project builds in one step. The crate's build.rs runs cbindgen
# and writes the C++ header into rust/osmflat-capi/include.
include(FetchContent)
FetchContent_Declare(
    Corrosion
    GIT_REPOSITORY https://github.com/corrosion-rs/corrosion.git
    GIT_TAG v0.5.1
)
FetchContent_MakeAvailable(Corrosion)

corrosion_import_crate(
    MANIFEST_PATH ${CMAKE_SOURCE_DIR}/rust/osmflat-capi/Cargo.toml
    PROFILE release
)

# --- Static render (cmake -DOSMFLAT_STATIC_RENDER=ON) ------------------------
# Builds Mapnik from source with osmflat compiled in, and a self-contained
# `render` linked against it -- instead of the osmflat.input module below.
option(OSMFLAT_STATIC_RENDER "Build a static Mapnik with osmflat built in, and link render against it" OFF)
if(OSMFLAT_STATIC_RENDER)
    include(cmake/StaticRender.cmake)
    return()
endif()

# --- Mapnik ------------------------------------------------------------------
find_package(PkgConfig REQUIRED)
# Mapnik 4 is required: its API returns std::optional where 3.x returned
# boost::optional (e.g. parameters::get), so a 3.x install would otherwise
# fail with compile errors deep in the sources rather than here.
pkg_check_modules(MAPNIK REQUIRED IMPORTED_TARGET libmapnik>=4.0)

# Mapnik's public headers include <boost/...>, but libmapnik.pc does not carry
# Boost's include path. On Homebrew, Boost is header-only under the brew prefix.
find_program(BREW_EXECUTABLE brew)
if(BREW_EXECUTABLE)
    execute_process(
        COMMAND ${BREW_EXECUTABLE} --prefix
        OUTPUT_VARIABLE HOMEBREW_PREFIX
        OUTPUT_STRIP_TRAILING_WHITESPACE
    )
endif()
if(NOT HOMEBREW_PREFIX)
    set(HOMEBREW_PREFIX /opt/homebrew)
endif()

# --- The plugin module -------------------------------------------------------
add_library(osmflat_input MODULE
    src/osmflat_datasource.cpp
    src/osmflat_featureset.cpp
    src/osmflat_dump_sink.cpp
    src/osmflat_group_hierarchy.cpp
)

# Mapnik loads plugins by the bare name `osmflat.input` (no `lib` prefix).
set_target_properties(osmflat_input PROPERTIES
    OUTPUT_NAME "osmflat"
    PREFIX ""
    SUFFIX ".input"
)

target_include_directories(osmflat_input PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/rust/osmflat-capi/include  # generated osmflat_capi.hpp
    ${HOMEBREW_PREFIX}/include                     # Boost (header-only)
)

# Linking osmflat_capi pulls in the Rust staticlib and its transitive system
# libs, and makes the module build wait for cargo (and thus the header) first.
target_link_libraries(osmflat_input PRIVATE
    osmflat_capi
    PkgConfig::MAPNIK
)

target_compile_options(osmflat_input PRIVATE -Wall -Wextra)

# Enables mapnik's MAPNIK_LOG_DEBUG/_WARN/_ERROR machinery (a no-op otherwise,
# per mapnik/debug.hpp) so per-query diagnostics are gated by the normal
# mapnik::logger severity instead of always-on or compiled away.
target_compile_definitions(osmflat_input PRIVATE MAPNIK_LOG)

# Place the built plugin where it's easy to point MAPNIK_PLUGINDIR at.
set_target_properties(osmflat_input PROPERTIES
    LIBRARY_OUTPUT_DIRECTORY ${CMAKE_BINARY_DIR}/plugins
)

# --- Optional smoke-test renderer (cmake -DBUILD_RENDER_TEST=ON) -------------
option(BUILD_RENDER_TEST "Build the standalone render smoke test" OFF)
if(BUILD_RENDER_TEST)
    add_executable(render test/render.cpp)
    target_include_directories(render PRIVATE ${HOMEBREW_PREFIX}/include)
    target_link_libraries(render PRIVATE PkgConfig::MAPNIK)
    add_dependencies(render osmflat_input)

    # Drives the osmflat and geojson.input plugins directly (no rendering) and
    # diffs the assembled polygon/multi_polygon features they return.
    add_executable(compare_plugins test/compare_plugins.cpp)
    target_include_directories(compare_plugins PRIVATE ${HOMEBREW_PREFIX}/include)
    target_link_libraries(compare_plugins PRIVATE PkgConfig::MAPNIK)
    add_dependencies(compare_plugins osmflat_input)

    # Drives the osmflat plugin's `member_of` param directly and checks its
    # output against a hardcoded ground-truth member list (no independent
    # plugin to cross-check against for relation membership).
    add_executable(check_member_of test/check_member_of.cpp)
    target_include_directories(check_member_of PRIVATE ${HOMEBREW_PREFIX}/include)
    target_link_libraries(check_member_of PRIVATE PkgConfig::MAPNIK)
    add_dependencies(check_member_of osmflat_input)

    # Drives the osmflat plugin's `name_lang` param directly and checks
    # resolved "name" values against the fixture's own name:<lang> tags.
    add_executable(check_name_lang test/check_name_lang.cpp)
    target_include_directories(check_name_lang PRIVATE ${HOMEBREW_PREFIX}/include)
    target_link_libraries(check_name_lang PRIVATE PkgConfig::MAPNIK)
    add_dependencies(check_name_lang osmflat_input)

    # Unit-tests the `group_hierarchy` rule engine in isolation (synthetic
    # features, no archive/bbox needed) -- compiles the module's source
    # directly rather than loading the plugin, since it's exercising
    # osmflat_group_hierarchy.cpp's own functions, not the datasource.
    add_executable(check_group_hierarchy test/check_group_hierarchy.cpp src/osmflat_group_hierarchy.cpp)
    target_include_directories(check_group_hierarchy PRIVATE
        ${CMAKE_SOURCE_DIR}/include
        ${HOMEBREW_PREFIX}/include
    )
    target_link_libraries(check_group_hierarchy PRIVATE PkgConfig::MAPNIK)

    # --- ctest registrations (`ctest --test-dir build`) ----------------------
    # The same invocations the README documents, against the checked-in
    # fixtures, so CI and local runs share one definition of "the tests".
    enable_testing()
    set(OSMFLAT_PLUGIN_DIR ${CMAKE_BINARY_DIR}/plugins)
    set(FIXTURES ${CMAKE_SOURCE_DIR}/test/fixtures)
    set(BAARLE_BBOX 4.75 51.38 5.02 51.49)

    # compare_plugins also needs mapnik's stock geojson.input; libmapnik.pc
    # advertises where the stock plugins live (Homebrew and Debian alike).
    pkg_get_variable(MAPNIK_PLUGINS_DIR libmapnik plugins_dir)

    add_test(NAME check_group_hierarchy
        COMMAND check_group_hierarchy ${CMAKE_SOURCE_DIR}/group-hierarchy.default.xml)
    add_test(NAME check_member_of
        COMMAND check_member_of ${OSMFLAT_PLUGIN_DIR} ${FIXTURES}/bicycle-route.osm.flat
            4.64 51.35 4.74 51.43)
    add_test(NAME check_name_lang
        COMMAND check_name_lang ${OSMFLAT_PLUGIN_DIR} ${FIXTURES}/baarle-hertog.osm.flat ${BAARLE_BBOX})
    add_test(NAME compare_plugins
        COMMAND compare_plugins ${OSMFLAT_PLUGIN_DIR} ${MAPNIK_PLUGINS_DIR}
            ${FIXTURES}/baarle-hertog.osm.flat ${FIXTURES}/baarle-hertog.geojson ${BAARLE_BBOX})
    # No-crash render smoke test (no pixel assertions; the style has no text,
    # so it doesn't depend on fonts being registered).
    add_test(NAME render_relations
        COMMAND render ${OSMFLAT_PLUGIN_DIR} ${CMAKE_SOURCE_DIR}/test/style-relations.xml
            ${FIXTURES}/baarle-hertog.osm.flat ${CMAKE_BINARY_DIR}/render_relations.png
            ${BAARLE_BBOX} 500 500)
endif()
