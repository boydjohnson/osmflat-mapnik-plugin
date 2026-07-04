#include "osmflat_datasource.hpp"
#include "osmflat_featureset.hpp"

#include <mapnik/datasource_plugin.hpp>
#include <mapnik/datasource_geometry_type.hpp>
#include <mapnik/feature_factory.hpp>
#include <mapnik/value/types.hpp>

#include <algorithm>
#include <tuple>

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

// Parses the comma-separated `tags` value into (key, value) prefilter terms.
// "key=value" -> {key,value}; "key" or "key=*" -> {key, ""} (key=*, any value).
static std::vector<std::pair<std::string, std::string>> parse_tag_filters(std::string const& spec)
{
    auto trim = [](std::string s) {
        s.erase(0, s.find_first_not_of(" \t"));
        auto e = s.find_last_not_of(" \t");
        return e == std::string::npos ? std::string() : s.substr(0, e + 1);
    };
    std::vector<std::pair<std::string, std::string>> out;
    std::size_t start = 0;
    while (start <= spec.size()) {
        std::size_t comma = spec.find(',', start);
        std::string tok = trim(spec.substr(start, comma == std::string::npos ? std::string::npos : comma - start));
        if (!tok.empty()) {
            std::size_t eq = tok.find('=');
            std::string key = trim(eq == std::string::npos ? tok : tok.substr(0, eq));
            std::string val = (eq == std::string::npos) ? std::string() : trim(tok.substr(eq + 1));
            if (val == "*") { val.clear(); }
            if (!key.empty()) { out.emplace_back(std::move(key), std::move(val)); }
        }
        if (comma == std::string::npos) { break; }
        start = comma + 1;
    }
    return out;
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

    // `tags`: comma-separated tag prefilter, e.g. "highway=motorway,building=*".
    // Pushed into the query via the Ext inverted index (needs the `ext` param).
    std::optional<std::string> tags = params.get<std::string>("tags");
    if (tags) {
        tag_filters_ = parse_tag_filters(*tags);
    }

    // `member_of`: relation-membership filter, e.g. "route=train,ref=Borealis".
    // Unlike `tags`, the terms AND together, and nodes/ways are emitted only as
    // members of a relation matching all of them (parent tags exposed as
    // `rel_*` attributes). Enforced even without `ext`, via a full relation
    // scan (slower).
    std::optional<std::string> member_of = params.get<std::string>("member_of");
    if (member_of) {
        member_of_filters_ = parse_tag_filters(*member_of);
    }

    // `ext`: optional Ext sidecar directory enabling the tag push-down.
    std::optional<std::string> ext = params.get<std::string>("ext");
    archive_ = std::make_shared<archive>(*file, ext ? *ext : std::string());

    auto e = archive_->envelope();
    extent_ = mapnik::box2d<double>(e[0], e[1], e[2], e[3]);

    // `numeric`: comma-separated tag keys to expose as numbers (opt-in), so
    // filters like `[lanes] > 2` compare numerically instead of as strings.
    std::optional<std::string> numeric = params.get<std::string>("numeric");
    if (numeric) {
        for (auto const& kv : parse_tag_filters(*numeric)) {
            numeric_keys_.insert(kv.first);   // reuse the CSV/key parser
        }
    }

    // `order`: draw order applied to returned features (default spatial).
    std::optional<std::string> order = params.get<std::string>("order");
    if (order) {
        if (*order == "z_order") { order_ = OsmflatOrder::OsmflatOrder_ZOrder; }
        else if (*order == "way_area") { order_ = OsmflatOrder::OsmflatOrder_WayArea; }
        else if (*order == "none") { order_ = OsmflatOrder::OsmflatOrder_None; }
        else {
            throw mapnik::datasource_exception("osmflat: 'order' must be z_order|way_area|none");
        }
    }

    // `simplify`: Douglas–Peucker tolerance in pixels (0 disables). Applied
    // scale-aware — the map-unit tolerance is derived per query from the
    // resolution, so geometry is generalized to sub-pixel at every zoom.
    std::optional<double> simplify = params.get<double>("simplify");
    if (simplify) {
        simplify_px_ = *simplify;
    }

    // Synthetic attributes always available; tag attributes are dynamic
    // (query-driven) and so are not advertised here.
    desc_.add_descriptor(mapnik::attribute_descriptor("osm_id", mapnik::Integer));
    desc_.add_descriptor(mapnik::attribute_descriptor("osm_type", mapnik::String));
    desc_.add_descriptor(mapnik::attribute_descriptor("is_closed", mapnik::Boolean));
    desc_.add_descriptor(mapnik::attribute_descriptor("way_area", mapnik::Double));
    desc_.add_descriptor(mapnik::attribute_descriptor("z_order", mapnik::Integer));
}

// Synthetic attribute names handled directly by the featureset; excluded from
// the tag keys sent to the query (otherwise they'd be double-handled).
static bool is_synthetic_attr(std::string const& name)
{
    return name == "osm_id" || name == "osm_type" || name == "is_closed"
        || name == "way_area" || name == "z_order";
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

// Borrowed views of the datasource's stable tag filters for the C API.
static std::vector<OsmflatKvRef> filter_refs(
    std::vector<std::pair<std::string, std::string>> const& filters)
{
    std::vector<OsmflatKvRef> refs;
    refs.reserve(filters.size());
    for (auto const& kv : filters) {
        refs.push_back(OsmflatKvRef{
            OsmflatStrRef{reinterpret_cast<const uint8_t*>(kv.first.data()), kv.first.size()},
            OsmflatStrRef{reinterpret_cast<const uint8_t*>(kv.second.data()), kv.second.size()}});
    }
    return refs;
}

mapnik::featureset_ptr osmflat_datasource::features(mapnik::query const& q) const
{
    mapnik::box2d<double> const& bbox = q.get_bbox();

    std::vector<std::string> keys = requested_keys(q);
    std::vector<OsmflatStrRef> refs = key_refs(keys);
    std::vector<OsmflatKvRef> filters = filter_refs(tag_filters_);
    std::vector<OsmflatKvRef> member_filters = filter_refs(member_of_filters_);

    // Scale-aware tolerance: `resolution` is pixels per map unit, so one pixel
    // is 1/res map units. Simplify at `simplify_px_` pixels.
    double tol = 0.0;
    if (simplify_px_ > 0.0) {
        auto const& res = q.resolution();
        double res_x = std::get<0>(res);
        double res_y = std::get<1>(res);
        double r = std::min(res_x, res_y);
        if (r > 0.0) {
            tol = simplify_px_ / r;
        }
    }

    feature_set fs = archive_->query(
        bbox.minx(), bbox.miny(), bbox.maxx(), bbox.maxy(),
        kinds_.nodes, kinds_.ways, kinds_.relations, refs, filters, member_filters,
        order_, tol);

    return std::make_shared<osmflat_featureset>(std::move(fs), std::move(keys), numeric_keys_);
}

mapnik::featureset_ptr osmflat_datasource::features_at_point(mapnik::coord2d const& pt, double tol) const
{
    // No property list on this path; emit synthetics only.
    std::vector<std::string> keys;
    std::vector<OsmflatStrRef> refs;
    std::vector<OsmflatKvRef> filters = filter_refs(tag_filters_);
    std::vector<OsmflatKvRef> member_filters = filter_refs(member_of_filters_);

    feature_set fs = archive_->query(
        pt.x - tol, pt.y - tol, pt.x + tol, pt.y + tol,
        kinds_.nodes, kinds_.ways, kinds_.relations, refs, filters, member_filters,
        order_, 0.0);   // no simplification for point queries

    return std::make_shared<osmflat_featureset>(std::move(fs), std::move(keys), numeric_keys_);
}

} // namespace osmflat

// Mapnik plugin registration: exports the symbols so that
// `<Parameter name="type">osmflat</Parameter>` resolves to this datasource.
DATASOURCE_PLUGIN_DEF(osmflat_datasource_plugin, osmflat);
DATASOURCE_PLUGIN_IMPL(osmflat_datasource_plugin, osmflat::osmflat_datasource);
DATASOURCE_PLUGIN_EXPORT(osmflat);
DATASOURCE_PLUGIN_EMPTY_AFTER_LOAD(osmflat_datasource_plugin);
DATASOURCE_PLUGIN_EMPTY_BEFORE_UNLOAD(osmflat_datasource_plugin);
