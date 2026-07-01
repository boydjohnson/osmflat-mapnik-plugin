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

# --- Mapnik ------------------------------------------------------------------
find_package(PkgConfig REQUIRED)
pkg_check_modules(MAPNIK REQUIRED IMPORTED_TARGET libmapnik)

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
endif()
