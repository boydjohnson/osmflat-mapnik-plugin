# Script mode (cmake -P): wire the osmflat datasource into a Mapnik source
# tree as a *built-in* plugin, so a static libmapnik (BUILD_SHARED_PLUGINS=OFF)
# carries it the same way it carries csv/geojson/shape.
#
#   cmake -DMAPNIK_SOURCE_DIR=<mapnik> -DOSMFLAT_SOURCE_DIR=<this repo> -P patch-mapnik.cmake
#
# Mapnik's static-plugin list is compile-time (datasource_cache_static.cpp +
# src/CMakeLists.txt), so there is no way to register an out-of-tree plugin
# without editing those files. Every edit anchors on an exact upstream string
# and fails loudly if the anchor is missing (i.e. Mapnik changed under us), and
# is skipped if already applied, so re-running on a populated tree is safe.

foreach(var MAPNIK_SOURCE_DIR OSMFLAT_SOURCE_DIR)
    if(NOT ${var})
        message(FATAL_ERROR "patch-mapnik.cmake: ${var} is required")
    endif()
endforeach()

function(osmflat_patch_file file anchor replacement marker)
    set(path "${MAPNIK_SOURCE_DIR}/${file}")
    file(READ "${path}" content)
    string(FIND "${content}" "${marker}" already)
    if(NOT already EQUAL -1)
        return()
    endif()
    string(FIND "${content}" "${anchor}" found)
    if(found EQUAL -1)
        message(FATAL_ERROR "patch-mapnik.cmake: anchor not found in ${file}:\n${anchor}")
    endif()
    string(REPLACE "${anchor}" "${replacement}" content "${content}")
    file(WRITE "${path}" "${content}")
endfunction()

# 1. The plugin directory: a CMakeLists that adds our sources to an
#    `input-osmflat` target, plus the header datasource_cache_static.cpp
#    includes to see `osmflat_datasource_plugin`.
set(plugin_dir "${MAPNIK_SOURCE_DIR}/plugins/input/osmflat")
file(MAKE_DIRECTORY "${plugin_dir}")
configure_file("${CMAKE_CURRENT_LIST_DIR}/plugin/CMakeLists.txt" "${plugin_dir}/CMakeLists.txt" COPYONLY)
configure_file("${CMAKE_CURRENT_LIST_DIR}/plugin/osmflat_datasource.hpp.in" "${plugin_dir}/osmflat_datasource.hpp" @ONLY)

# 2. Build it alongside the stock plugins.
osmflat_patch_file(plugins/input/CMakeLists.txt
    "if(USE_PLUGIN_INPUT_TILES)"
    "if(TARGET osmflat_capi)\n    add_subdirectory(osmflat)\n    list(APPEND m_build_plugins input-osmflat)\nendif()\nif(USE_PLUGIN_INPUT_TILES)"
    "add_subdirectory(osmflat)")

# 3. Link its sources into libmapnik and define MAPNIK_STATIC_PLUGIN_OSMFLAT.
osmflat_patch_file(src/CMakeLists.txt
    "    $<$<AND:$<NOT:$<BOOL:\${BUILD_SHARED_PLUGINS}>>,$<TARGET_EXISTS:input-tiles>>:input-tiles>\n"
    "    $<$<AND:$<NOT:$<BOOL:\${BUILD_SHARED_PLUGINS}>>,$<TARGET_EXISTS:input-tiles>>:input-tiles>\n    $<$<AND:$<NOT:$<BOOL:\${BUILD_SHARED_PLUGINS}>>,$<TARGET_EXISTS:input-osmflat>>:input-osmflat>\n"
    "TARGET_EXISTS:input-osmflat>>:input-osmflat>")
osmflat_patch_file(src/CMakeLists.txt
    "    $<$<AND:$<NOT:$<BOOL:\${BUILD_SHARED_PLUGINS}>>,$<TARGET_EXISTS:input-tiles>>:MAPNIK_STATIC_PLUGIN_TILES>\n"
    "    $<$<AND:$<NOT:$<BOOL:\${BUILD_SHARED_PLUGINS}>>,$<TARGET_EXISTS:input-tiles>>:MAPNIK_STATIC_PLUGIN_TILES>\n    $<$<AND:$<NOT:$<BOOL:\${BUILD_SHARED_PLUGINS}>>,$<TARGET_EXISTS:input-osmflat>>:MAPNIK_STATIC_PLUGIN_OSMFLAT>\n"
    "MAPNIK_STATIC_PLUGIN_OSMFLAT")

# 4. Accept "+proj=longlat ..." without PROJ, classified the way PROJ
#    classifies it.
#
#    Built with PROJ, `proj_get_type` reports a proj4 longlat string as
#    PJ_TYPE_OTHER_CRS, so `is_geographic_` stays false and scale denominators
#    come out in degrees -- which is what every style written against this
#    plugin is tuned for (a MaxScaleDenominator of 0.1 ~ neighborhood). Without
#    PROJ, Mapnik knows only "epsg:4326" (geographic, so ~111319x larger
#    denominators) and throws on anything else. Matching PROJ's classification
#    here keeps styles renderer-independent; other proj4 strings still throw.
osmflat_patch_file(src/projection.cpp
[==[#ifdef MAPNIK_USE_PROJ
        init_proj();
#else
        throw std::runtime_error(std::string("Cannot initialize projection '") + params_ +
                                 " ' without proj support (-DMAPNIK_USE_PROJ)");
#endif]==]
[==[#ifdef MAPNIK_USE_PROJ
        init_proj();
#else
        // osmflat: PROJ reports a proj4 longlat string as a non-geographic
        // "other" CRS; mirror that so scale denominators stay in degrees.
        if (params_.rfind("+proj=longlat", 0) == 0)
        {
            is_geographic_ = false;
        }
        else
        {
            throw std::runtime_error(std::string("Cannot initialize projection '") + params_ +
                                     " ' without proj support (-DMAPNIK_USE_PROJ)");
        }
#endif]==]
[==[osmflat: PROJ reports a proj4 longlat string]==])

# 5. Register it in the static datasource table.
osmflat_patch_file(src/datasource_cache_static.cpp
    "#if defined(MAPNIK_STATIC_PLUGIN_TOPOJSON)\n#include \"input/topojson/topojson_datasource.hpp\"\n#endif\n"
    "#if defined(MAPNIK_STATIC_PLUGIN_TOPOJSON)\n#include \"input/topojson/topojson_datasource.hpp\"\n#endif\n#if defined(MAPNIK_STATIC_PLUGIN_OSMFLAT)\n#include \"input/osmflat/osmflat_datasource.hpp\"\n#endif\n"
    "input/osmflat/osmflat_datasource.hpp")
osmflat_patch_file(src/datasource_cache_static.cpp
    "    REGISTER_STATIC_DATASOURCE_PLUGIN(topojson_datasource_plugin);\n#endif\n"
    "    REGISTER_STATIC_DATASOURCE_PLUGIN(topojson_datasource_plugin);\n#endif\n#if defined(MAPNIK_STATIC_PLUGIN_OSMFLAT)\n    REGISTER_STATIC_DATASOURCE_PLUGIN(osmflat_datasource_plugin);\n#endif\n"
    "REGISTER_STATIC_DATASOURCE_PLUGIN(osmflat_datasource_plugin)")
