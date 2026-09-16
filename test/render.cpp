// Standalone smoke test: loads the built `osmflat.input` plugin through
// mapnik's datasource_cache (the real runtime path), loads a Mapnik XML style,
// and renders a PNG over a bounding box.
//
//   render <plugin_dir> <style.xml> <archive_dir> <out.png>
//          <minx> <miny> <maxx> <maxy>
//
// The token @ARCHIVE@ in the style is replaced with <archive_dir>.

#include <mapnik/mapnik.hpp>
#include <mapnik/map.hpp>
#include <mapnik/debug.hpp>
#include <mapnik/font_engine_freetype.hpp>
#include <mapnik/datasource_cache.hpp>
#include <mapnik/load_map.hpp>
#include <mapnik/image.hpp>
#include <mapnik/image_util.hpp>
#include <mapnik/agg_renderer.hpp>
#include <mapnik/geometry/box2d.hpp>

#include <cstdlib>
#include <fstream>
#include <iostream>
#include <sstream>
#include <string>

static std::string slurp(const std::string& path)
{
    std::ifstream in(path);
    std::ostringstream ss;
    ss << in.rdbuf();
    return ss.str();
}

int main(int argc, char** argv)
{
    if (argc != 9 && argc != 11) {
        std::cerr << "usage: render <plugin_dir> <style.xml> <archive_dir> "
                     "<out.png> <minx> <miny> <maxx> <maxy> [width height]\n";
        return EXIT_FAILURE;
    }
    const std::string plugin_dir = argv[1];
    const std::string style_xml = argv[2];
    const std::string archive_dir = argv[3];
    const std::string out_png = argv[4];
    const double minx = std::stod(argv[5]);
    const double miny = std::stod(argv[6]);
    const double maxx = std::stod(argv[7]);
    const double maxy = std::stod(argv[8]);
    const int width = (argc == 11) ? std::stoi(argv[9]) : 1000;
    const int height = (argc == 11) ? std::stoi(argv[10]) : 1000;

    try {
        // Opt-in: OSMFLAT_LOG_DEBUG=1 surfaces the plugin's per-query
        // MAPNIK_LOG_DEBUG lines (bbox/tags/osm_type asked for, feature count
        // returned) on stderr. Off by default so ordinary renders stay quiet.
        if (std::getenv("OSMFLAT_LOG_DEBUG")) {
            mapnik::logger::set_severity(mapnik::logger::debug);
        }

        mapnik::setup();
        mapnik::datasource_cache::instance().register_datasources(plugin_dir);
        // Register bundled DejaVu fonts so TextSymbolizer can resolve face-name.
        if (const char* fd = std::getenv("MAPNIK_FONT_DIR")) {
            mapnik::freetype_engine::register_fonts(fd, true);
        } else {
            mapnik::freetype_engine::register_fonts("/opt/homebrew/lib/mapnik/fonts", true);
        }

        std::string xml = slurp(style_xml);
        for (std::string::size_type p; (p = xml.find("@ARCHIVE@")) != std::string::npos;) {
            xml.replace(p, std::string("@ARCHIVE@").size(), archive_dir);
        }
        // @EXT@ -> Ext sidecar dir: $OSMFLAT_EXT, else derive by swapping a
        // trailing ".flat" for ".ext" (mexico.osm.flat -> mexico.osm.ext).
        std::string ext_dir;
        if (const char* e = std::getenv("OSMFLAT_EXT")) {
            ext_dir = e;
        } else if (archive_dir.size() >= 5 && archive_dir.substr(archive_dir.size() - 5) == ".flat") {
            ext_dir = archive_dir.substr(0, archive_dir.size() - 5) + ".ext";
        }
        for (std::string::size_type p; (p = xml.find("@EXT@")) != std::string::npos;) {
            xml.replace(p, std::string("@EXT@").size(), ext_dir);
        }

        mapnik::Map m(width, height);
        mapnik::load_map_string(m, xml);

        m.zoom_to_box(mapnik::box2d<double>(minx, miny, maxx, maxy));
        std::cerr << "scale_denominator=" << m.scale_denominator() << "\n";

        mapnik::image_rgba8 im(m.width(), m.height());
        mapnik::agg_renderer<mapnik::image_rgba8> ren(m, im);
        ren.apply();
        mapnik::save_to_file(im, out_png, "png");

        std::cout << "rendered " << out_png << "\n";
    } catch (std::exception const& ex) {
        std::cerr << "exception: " << ex.what() << "\n";
        return EXIT_FAILURE;
    }
    return EXIT_SUCCESS;
}
