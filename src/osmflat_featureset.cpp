#include "osmflat_featureset.hpp"

#include <mapnik/feature_factory.hpp>
#include <mapnik/geometry.hpp>
#include <mapnik/value/types.hpp>

namespace osmflat {

namespace {

const char* osm_type_name(OsmflatOsmType t)
{
    switch (t) {
        case OsmflatOsmType::OsmflatOsmType_Way: return "way";
        case OsmflatOsmType::OsmflatOsmType_Relation: return "relation";
        case OsmflatOsmType::OsmflatOsmType_Node:
        default: return "node";
    }
}

} // namespace

osmflat_featureset::osmflat_featureset(feature_set&& fs, std::vector<std::string> keys)
    : fs_(std::move(fs)), keys_(std::move(keys))
{
    // Fixed schema shared by every feature in this query.
    ctx_ = std::make_shared<mapnik::context_type>();
    ctx_->push("osm_id");
    ctx_->push("osm_type");
    ctx_->push("is_closed");
    for (auto const& k : keys_) {
        ctx_->push(k);
    }
}

mapnik::feature_ptr osmflat_featureset::next()
{
    if (!fs_.next()) {
        return mapnik::feature_ptr();
    }

    mapnik::feature_ptr feature = mapnik::feature_factory::create(ctx_, ++feature_id_);

    // Geometry.
    std::size_t n = fs_.num_coords();
    const double* coords = fs_.coords();
    switch (fs_.geom_type()) {
        case OsmflatGeomType::OsmflatGeomType_Point: {
            if (n >= 1) {
                feature->set_geometry(mapnik::geometry::point<double>(coords[0], coords[1]));
            }
            break;
        }
        case OsmflatGeomType::OsmflatGeomType_LineString:
        case OsmflatGeomType::OsmflatGeomType_Polygon: {
            mapnik::geometry::line_string<double> line;
            line.reserve(n);
            for (std::size_t i = 0; i < n; ++i) {
                line.emplace_back(coords[2 * i], coords[2 * i + 1]);
            }
            feature->set_geometry(std::move(line));
            break;
        }
    }

    // Synthetic geometric-fact attributes (always present).
    uint64_t osm_id = 0;
    if (fs_.osm_id(osm_id)) {
        feature->put("osm_id", static_cast<mapnik::value_integer>(osm_id));
    } else {
        feature->put("osm_id", mapnik::value_null());
    }
    feature->put("osm_type", mapnik::value_unicode_string::fromUTF8(osm_type_name(fs_.osm_type())));
    feature->put("is_closed", static_cast<mapnik::value_bool>(fs_.is_closed()));

    // Requested tags: value or null, aligned to keys_.
    std::string value;
    for (std::size_t i = 0; i < keys_.size(); ++i) {
        if (fs_.attr(i, value)) {
            feature->put(keys_[i], mapnik::value_unicode_string::fromUTF8(value));
        } else {
            feature->put(keys_[i], mapnik::value_null());
        }
    }

    return feature;
}

} // namespace osmflat
