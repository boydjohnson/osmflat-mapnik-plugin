#include "osmflat_featureset.hpp"

#include <mapnik/feature_factory.hpp>
#include <mapnik/geometry.hpp>
#include <mapnik/value/types.hpp>

#include <cerrno>
#include <cstdlib>

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

// Puts a value coerced to a number under `key`: an integer when it parses
// exactly, else a double, else null (a `numeric` key whose value isn't numeric,
// e.g. "50 mph"). Mirrors PostGIS `col::int` semantics for filters like
// `[lanes] > 2`.
void put_numeric(mapnik::feature_ptr const& feature, std::string const& key, std::string const& value)
{
    const char* begin = value.c_str();
    char* end = nullptr;

    errno = 0;
    long long as_int = std::strtoll(begin, &end, 10);
    if (end == begin + value.size() && errno == 0) {
        feature->put(key, static_cast<mapnik::value_integer>(as_int));
        return;
    }

    errno = 0;
    double as_double = std::strtod(begin, &end);
    if (end == begin + value.size() && errno == 0) {
        feature->put(key, static_cast<mapnik::value_double>(as_double));
        return;
    }

    feature->put(key, mapnik::value_null());
}

} // namespace

osmflat_featureset::osmflat_featureset(feature_set&& fs, std::vector<std::string> keys,
                                       std::size_t style_key_count,
                                       std::vector<std::string> name_langs,
                                       std::set<std::string> numeric_keys,
                                       std::shared_ptr<dump_sink> dump)
    : fs_(std::move(fs)), keys_(std::move(keys)), style_key_count_(style_key_count),
      name_langs_(std::move(name_langs)),
      numeric_keys_(std::move(numeric_keys)), dump_(std::move(dump))
{
    // Fixed schema shared by every feature in this query. Only the
    // style-requested keys become mapnik attributes; any dump-only keys past
    // style_key_count_ are fetched for the dump record alone.
    ctx_ = std::make_shared<mapnik::context_type>();
    ctx_->push("osm_id");
    ctx_->push("osm_type");
    ctx_->push("is_closed");
    ctx_->push("way_area");
    ctx_->push("z_order");
    for (std::size_t i = 0; i < style_key_count_; ++i) {
        ctx_->push(keys_[i]);
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
        case OsmflatGeomType::OsmflatGeomType_MultiPolygon: {
            mapnik::geometry::multi_polygon<double> mp;
            std::size_t np = fs_.num_polygons();
            mp.reserve(np);
            for (std::size_t p = 0; p < np; ++p) {
                mapnik::geometry::polygon<double> poly;
                std::size_t nr = fs_.polygon_num_rings(p);
                for (std::size_t r = 0; r < nr; ++r) {
                    std::size_t rn = fs_.ring_num_coords(p, r);
                    const double* rc = fs_.ring_coords(p, r);
                    mapnik::geometry::linear_ring<double> ring;
                    ring.reserve(rn);
                    for (std::size_t i = 0; i < rn; ++i) {
                        ring.emplace_back(rc[2 * i], rc[2 * i + 1]);
                    }
                    poly.push_back(std::move(ring));   // ring 0 = exterior, rest = holes
                }
                mp.push_back(std::move(poly));
            }
            feature->set_geometry(std::move(mp));
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
    feature->put("way_area", static_cast<mapnik::value_double>(fs_.way_area()));
    feature->put("z_order", static_cast<mapnik::value_integer>(fs_.z_order()));

    // Requested tags: value or null, aligned to keys_. Keys listed in the
    // datasource `numeric` param are coerced to numbers so filters compare
    // numerically (e.g. [lanes] > 2). Only the style-requested prefix becomes
    // a mapnik attribute; any dump-only keys are read separately below.
    std::string value;
    std::string plain_name;
    bool have_plain_name = false;
    for (std::size_t i = 0; i < style_key_count_; ++i) {
        if (!fs_.attr(i, value)) {
            feature->put(keys_[i], mapnik::value_null());
        } else if (numeric_keys_.count(keys_[i])) {
            put_numeric(feature, keys_[i], value);
        } else {
            feature->put(keys_[i], mapnik::value_unicode_string::fromUTF8(value));
            if (keys_[i] == "name") {
                plain_name = value;
                have_plain_name = !value.empty();
            }
        }
    }

    // Language resolution (only active when the style requested [name] *and*
    // `name_lang` was configured -- see osmflat_datasource::features(), which
    // leaves name_langs_ empty otherwise). Walk the requested languages in
    // priority order: "_" checks the plain tag already put above; anything
    // else checks the matching "name:<lang>" candidate osmflat_datasource
    // appended at keys_[style_key_count_, ...), in the same relative order as
    // the non-"_" entries in name_langs_. The first non-empty hit wins. If
    // nothing matches, "name" resolves to null -- mirroring an unmatched SQL
    // `coalesce(...)` -- rather than silently keeping the plain tag.
    if (!name_langs_.empty()) {
        bool resolved = false;
        std::size_t candidate = style_key_count_;
        for (std::string const& lang : name_langs_) {
            if (lang == "_") {
                if (have_plain_name) {
                    feature->put("name", mapnik::value_unicode_string::fromUTF8(plain_name));
                    resolved = true;
                    break;
                }
                continue;
            }
            if (fs_.attr(candidate, value) && !value.empty()) {
                feature->put("name", mapnik::value_unicode_string::fromUTF8(value));
                resolved = true;
                break;
            }
            ++candidate;
        }
        if (!resolved) {
            feature->put("name", mapnik::value_null());
        }
    }

    if (dump_) {
        write_dump_record();
    }

    return feature;
}

void osmflat_featureset::write_dump_record()
{
    dump_record rec;
    rec.osm_type = osm_type_name(fs_.osm_type());
    uint64_t osm_id = 0;
    rec.has_osm_id = fs_.osm_id(osm_id);
    rec.osm_id = osm_id;
    rec.z_order = fs_.z_order();
    rec.way_area = fs_.way_area();
    rec.is_closed = fs_.is_closed();

    switch (fs_.geom_type()) {
        case OsmflatGeomType::OsmflatGeomType_Point: {
            rec.geom_type = "Point";
            std::size_t n = fs_.num_coords();
            const double* coords = fs_.coords();
            if (n >= 1) {
                rec.ring.emplace_back(coords[0], coords[1]);
            }
            break;
        }
        case OsmflatGeomType::OsmflatGeomType_MultiPolygon: {
            rec.geom_type = "MultiPolygon";
            std::size_t np = fs_.num_polygons();
            rec.polygons.reserve(np);
            for (std::size_t p = 0; p < np; ++p) {
                std::vector<std::vector<std::pair<double, double>>> poly;
                std::size_t nr = fs_.polygon_num_rings(p);
                poly.reserve(nr);
                for (std::size_t r = 0; r < nr; ++r) {
                    std::size_t rn = fs_.ring_num_coords(p, r);
                    const double* rc = fs_.ring_coords(p, r);
                    std::vector<std::pair<double, double>> ring;
                    ring.reserve(rn);
                    for (std::size_t i = 0; i < rn; ++i) {
                        ring.emplace_back(rc[2 * i], rc[2 * i + 1]);
                    }
                    poly.push_back(std::move(ring));
                }
                rec.polygons.push_back(std::move(poly));
            }
            break;
        }
        case OsmflatGeomType::OsmflatGeomType_LineString:
        case OsmflatGeomType::OsmflatGeomType_Polygon:
        default: {
            rec.geom_type = "LineString";
            std::size_t n = fs_.num_coords();
            const double* coords = fs_.coords();
            rec.ring.reserve(n);
            for (std::size_t i = 0; i < n; ++i) {
                rec.ring.emplace_back(coords[2 * i], coords[2 * i + 1]);
            }
            break;
        }
    }

    // Dump-only tags live past style_key_count_; style tags are re-read here
    // too so the dump record carries a feature's full known identity even
    // when a naming tag also happens to be style-requested.
    std::string value;
    rec.tags.reserve(keys_.size());
    for (std::size_t i = 0; i < keys_.size(); ++i) {
        if (fs_.attr(i, value)) {
            rec.tags.emplace_back(keys_[i], value);
        }
    }

    dump_->write(rec);
}

} // namespace osmflat
