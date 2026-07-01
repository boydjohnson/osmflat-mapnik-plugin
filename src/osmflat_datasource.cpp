#include "osmflat_datasource.hpp"
#include "osmflat_featureset.hpp"

#include <mapnik/datasource_plugin.hpp>
#include <mapnik/datasource_geometry_type.hpp>
#include <mapnik/feature_factory.hpp>
#include <mapnik/value/types.hpp>

#include <stdexcept>

namespace osmflat {

const std::string osmflat_datasource::name_ = "osmflat";

osmflat_datasource::osmflat_datasource(mapnik::parameters const& params)
    : mapnik::datasource(params),
      desc_(name_, "EPSG:4326"),
      kind_(query_kind::all)
{
    init(params);
}

void osmflat_datasource::init(mapnik::parameters const& params)
{
    std::optional<std::string> file = params.get<std::string>("file");
    if (!file) {
        throw mapnik::datasource_exception("osmflat: missing required parameter 'file' (archive directory)");
    }

    // `osm_type`: node | way | all (default all).
    std::optional<std::string> osm_type = params.get<std::string>("osm_type");
    if (osm_type) {
        if (*osm_type == "node") { kind_ = query_kind::nodes; }
        else if (*osm_type == "way") { kind_ = query_kind::ways; }
        else if (*osm_type == "all") { kind_ = query_kind::all; }
        else {
            throw mapnik::datasource_exception("osmflat: 'osm_type' must be one of node|way|all");
        }
    }

    archive_ = std::make_shared<archive>(*file);

    auto e = archive_->envelope();
    extent_ = mapnik::box2d<double>(e[0], e[1], e[2], e[3]);

    // Synthetic geometric-fact attributes always available; tag attributes are
    // dynamic (query-driven) and so are not advertised here.
    desc_.add_descriptor(mapnik::attribute_descriptor("osm_id", mapnik::Integer));
    desc_.add_descriptor(mapnik::attribute_descriptor("osm_type", mapnik::String));
    desc_.add_descriptor(mapnik::attribute_descriptor("is_closed", mapnik::Boolean));
}

// Synthetic attribute names handled directly by the featureset; excluded from
// the tag keys sent to the query.
static bool is_synthetic_attr(std::string const& name)
{
    return name == "osm_id" || name == "osm_type" || name == "is_closed";
}

// Builds the ordered list of requested tag names from the query's referenced
// property names, dropping the synthetic ones.
static std::vector<std::string> requested_keys(mapnik::query const& q)
{
    std::vector<std::string> keys;
    for (std::string const& name : q.property_names()) {
        if (!is_synthetic_attr(name)) {
            keys.push_back(name);
        }
    }
    return keys;
}

mapnik::datasource::datasource_t osmflat_datasource::type() const
{
    return mapnik::datasource::Vector;
}

const char* osmflat_datasource::name()
{
    return name_.c_str();
}

mapnik::box2d<double> osmflat_datasource::envelope() const
{
    return extent_;
}

std::optional<mapnik::datasource_geometry_t> osmflat_datasource::get_geometry_type() const
{
    // Mixed point/line content; let mapnik infer per-feature.
    return std::nullopt;
}

mapnik::layer_descriptor osmflat_datasource::get_descriptor() const
{
    return desc_;
}

// Borrowed views of `keys` for the C API; the returned refs point into the
// argument's strings, which must outlive the query call.
static std::vector<OsmflatStrRef> key_refs(std::vector<std::string> const& keys)
{
    std::vector<OsmflatStrRef> refs;
    refs.reserve(keys.size());
    for (std::string const& k : keys) {
        refs.push_back(OsmflatStrRef{reinterpret_cast<const uint8_t*>(k.data()), k.size()});
    }
    return refs;
}

mapnik::featureset_ptr osmflat_datasource::features(mapnik::query const& q) const
{
    mapnik::box2d<double> const& bbox = q.get_bbox();

    bool include_nodes = (kind_ == query_kind::nodes || kind_ == query_kind::all);
    bool include_ways = (kind_ == query_kind::ways || kind_ == query_kind::all);

    std::vector<std::string> keys = requested_keys(q);
    std::vector<OsmflatStrRef> refs = key_refs(keys);

    feature_set fs = archive_->query(
        bbox.minx(), bbox.miny(), bbox.maxx(), bbox.maxy(),
        include_nodes, include_ways, refs);

    return std::make_shared<osmflat_featureset>(std::move(fs), std::move(keys));
}

mapnik::featureset_ptr osmflat_datasource::features_at_point(mapnik::coord2d const& pt, double tol) const
{
    bool include_nodes = (kind_ == query_kind::nodes || kind_ == query_kind::all);
    bool include_ways = (kind_ == query_kind::ways || kind_ == query_kind::all);

    // No property list on this path; emit synthetics only.
    std::vector<std::string> keys;
    std::vector<OsmflatStrRef> refs;

    feature_set fs = archive_->query(
        pt.x - tol, pt.y - tol, pt.x + tol, pt.y + tol,
        include_nodes, include_ways, refs);

    return std::make_shared<osmflat_featureset>(std::move(fs), std::move(keys));
}

} // namespace osmflat

// Mapnik plugin registration: exports the symbols so that
// `<Parameter name="type">osmflat</Parameter>` resolves to this datasource.
DATASOURCE_PLUGIN_DEF(osmflat_datasource_plugin, osmflat);
DATASOURCE_PLUGIN_IMPL(osmflat_datasource_plugin, osmflat::osmflat_datasource);
DATASOURCE_PLUGIN_EXPORT(osmflat);
DATASOURCE_PLUGIN_EMPTY_AFTER_LOAD(osmflat_datasource_plugin);
DATASOURCE_PLUGIN_EMPTY_BEFORE_UNLOAD(osmflat_datasource_plugin);
