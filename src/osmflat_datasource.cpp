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
      desc_(name_, "EPSG:4326")
{
    init(params);
}

// Parses the comma-separated `osm_type` value (e.g. "way,relation") into the
// set of primitives to emit. "all" (the default) enables everything.
static query_kinds parse_kinds(std::string const& spec)
{
    query_kinds k{false, false, false};
    std::size_t start = 0;
    while (start <= spec.size()) {
        std::size_t comma = spec.find(',', start);
        std::string tok = spec.substr(start, comma == std::string::npos ? std::string::npos : comma - start);
        // trim spaces
        tok.erase(0, tok.find_first_not_of(" \t"));
        tok.erase(tok.find_last_not_of(" \t") + 1);
        if (tok == "all") { k = query_kinds{true, true, true}; }
        else if (tok == "node") { k.nodes = true; }
        else if (tok == "way") { k.ways = true; }
        else if (tok == "relation") { k.relations = true; }
        else if (!tok.empty()) {
            throw mapnik::datasource_exception("osmflat: 'osm_type' tokens must be node|way|relation|all");
        }
        if (comma == std::string::npos) { break; }
        start = comma + 1;
    }
    return k;
}

void osmflat_datasource::init(mapnik::parameters const& params)
{
    std::optional<std::string> file = params.get<std::string>("file");
    if (!file) {
        throw mapnik::datasource_exception("osmflat: missing required parameter 'file' (archive directory)");
    }

    // `osm_type`: comma-separated node|way|relation|all (default all).
    std::optional<std::string> osm_type = params.get<std::string>("osm_type");
    if (osm_type) {
        kinds_ = parse_kinds(*osm_type);
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

    std::vector<std::string> keys = requested_keys(q);
    std::vector<OsmflatStrRef> refs = key_refs(keys);

    feature_set fs = archive_->query(
        bbox.minx(), bbox.miny(), bbox.maxx(), bbox.maxy(),
        kinds_.nodes, kinds_.ways, kinds_.relations, refs);

    return std::make_shared<osmflat_featureset>(std::move(fs), std::move(keys));
}

mapnik::featureset_ptr osmflat_datasource::features_at_point(mapnik::coord2d const& pt, double tol) const
{
    // No property list on this path; emit synthetics only.
    std::vector<std::string> keys;
    std::vector<OsmflatStrRef> refs;

    feature_set fs = archive_->query(
        pt.x - tol, pt.y - tol, pt.x + tol, pt.y + tol,
        kinds_.nodes, kinds_.ways, kinds_.relations, refs);

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
