# Copied into <mapnik>/plugins/input/osmflat by patch-mapnik.cmake.
#
# With BUILD_SHARED_PLUGINS=OFF, add_plugin_target makes an INTERFACE library
# whose sources compile into libmapnik itself. OSMFLAT_SOURCE_DIR comes from the
# parent project (cmake/StaticRender.cmake). Paths are wrapped in
# $<BUILD_INTERFACE:> so Mapnik's install(EXPORT) never sees source-tree paths.
add_plugin_target(input-osmflat "osmflat")

target_sources(input-osmflat ${_plugin_visibility}
    $<BUILD_INTERFACE:${OSMFLAT_SOURCE_DIR}/src/osmflat_datasource.cpp>
    $<BUILD_INTERFACE:${OSMFLAT_SOURCE_DIR}/src/osmflat_featureset.cpp>
    $<BUILD_INTERFACE:${OSMFLAT_SOURCE_DIR}/src/osmflat_dump_sink.cpp>
    $<BUILD_INTERFACE:${OSMFLAT_SOURCE_DIR}/src/osmflat_group_hierarchy.cpp>
)
target_include_directories(input-osmflat ${_plugin_visibility}
    $<BUILD_INTERFACE:${OSMFLAT_SOURCE_DIR}/include>
    $<BUILD_INTERFACE:${OSMFLAT_SOURCE_DIR}/rust/osmflat-capi/include>
)
target_link_libraries(input-osmflat ${_plugin_visibility}
    mapnik::mapnik
)
