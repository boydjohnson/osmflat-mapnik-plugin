// Standalone smoke test: loads the built `osmflat.input` plugin through
// mapnik's datasource_cache (the real runtime path), queries an archive over a
// bounding box, and renders a PNG.
//
//   render <archive_dir> <plugin_dir> <out.png> <minx> <miny> <maxx> <maxy>

#include <mapnik/mapnik.hpp>
#include <mapnik/map.hpp>
#include <mapnik/layer.hpp>
#include <mapnik/rule.hpp>
#include <mapnik/feature_type_style.hpp>
#include <mapnik/symbolizer.hpp>
#include <mapnik/datasource.hpp>
#include <mapnik/datasource_cache.hpp>
#include <mapnik/params.hpp>
#include <mapnik/image.hpp>
#include <mapnik/image_util.hpp>
#include <mapnik/agg_renderer.hpp>
#include <mapnik/color.hpp>

#include <cstdlib>
#include <iostream>
#include <string>

int main(int argc, char** argv)
{
    if (argc != 8) {
        std::cerr << "usage: render <archive_dir> <plugin_dir> <out.png> "
                     "<minx> <miny> <maxx> <maxy>\n";
        return EXIT_FAILURE;
    }
    const std::string archive_dir = argv[1];
    const std::string plugin_dir = argv[2];
    const std::string out_png = argv[3];
    const double minx = std::stod(argv[4]);
    const double miny = std::stod(argv[5]);
    const double maxx = std::stod(argv[6]);
    const double maxy = std::stod(argv[7]);

    try {
        mapnik::setup();
        // Register only our plugin directory -- proves the .input module loads.
        mapnik::datasource_cache::instance().register_datasources(plugin_dir);

        mapnik::Map m(1000, 1000);
        m.set_background(mapnik::color("white"));

        mapnik::parameters params;
        params["type"] = "osmflat";
        params["file"] = archive_dir;
        params["osm_type"] = "way";

        auto ds = mapnik::datasource_cache::instance().create(params);

        mapnik::layer lyr("osmflat", "EPSG:4326");
        lyr.set_datasource(ds);
        lyr.add_style("lines");

        mapnik::rule r;
        mapnik::line_symbolizer line;
        mapnik::put(line, mapnik::keys::stroke, mapnik::color("#333333"));
        mapnik::put(line, mapnik::keys::stroke_width, 1.0);
        r.append(std::move(line));

        mapnik::feature_type_style st;
        st.add_rule(std::move(r));
        m.insert_style("lines", std::move(st));
        m.add_layer(lyr);

        m.zoom_to_box(mapnik::box2d<double>(minx, miny, maxx, maxy));

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
