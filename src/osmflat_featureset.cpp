#include "osmflat_featureset.hpp"

#include <mapnik/feature_factory.hpp>
#include <mapnik/geometry.hpp>
#include <mapnik/value/types.hpp>

namespace osmflat {

osmflat_featureset::osmflat_featureset(feature_set&& fs)
    : fs_(std::move(fs))
{
}

mapnik::feature_ptr osmflat_featureset::next()
{
    if (!fs_.next()) {
        return mapnik::feature_ptr();
    }

    // Fresh context per feature so its keys match its values exactly.
    mapnik::context_ptr ctx = std::make_shared<mapnik::context_type>();
    ctx->push("osm_id");

    mapnik::feature_ptr feature =
        mapnik::feature_factory::create(ctx, static_cast<mapnik::value_integer>(fs_.id()));

    feature->put("osm_id", static_cast<mapnik::value_integer>(fs_.id()));

    std::size_t n = fs_.num_coords();
    const double* coords = fs_.coords();

    switch (fs_.geom_type()) {
        case OsmflatGeomType::OsmflatGeomType_Point: {
            if (n >= 1) {
                mapnik::geometry::point<double> pt(coords[0], coords[1]);
                feature->set_geometry(std::move(pt));
            }
            break;
        }
        case OsmflatGeomType::OsmflatGeomType_LineString: {
            mapnik::geometry::line_string<double> line;
            line.reserve(n);
            for (std::size_t i = 0; i < n; ++i) {
                line.emplace_back(coords[2 * i], coords[2 * i + 1]);
            }
            feature->set_geometry(std::move(line));
            break;
        }
    }

    // Attach tags as attributes, lazily registering each new key in the shared
    // context so mapnik can resolve them by name.
    std::size_t num_tags = fs_.num_tags();
    for (std::size_t i = 0; i < num_tags; ++i) {
        std::string key = fs_.tag_key(i);
        if (key.empty()) { continue; }
        ctx->push(key);
        feature->put_new(key, mapnik::value_unicode_string::fromUTF8(fs_.tag_value(i)));
    }

    return feature;
}

} // namespace osmflat
