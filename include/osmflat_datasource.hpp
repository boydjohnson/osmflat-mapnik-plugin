#ifndef OSMFLAT_DATASOURCE_HPP
#define OSMFLAT_DATASOURCE_HPP

#include <mapnik/datasource.hpp>
#include <mapnik/params.hpp>
#include <mapnik/query.hpp>
#include <mapnik/feature.hpp>
#include <mapnik/feature_layer_desc.hpp>
#include <mapnik/geometry/box2d.hpp>
#include <mapnik/coord.hpp>

#include <memory>
#include <set>
#include <string>
#include <utility>
#include <vector>

#include "osmflat_archive.hpp"
#include "osmflat_dump_sink.hpp"
#include "osmflat_group_hierarchy.hpp"

namespace osmflat {

/// Which OSM primitive types the datasource emits (a query-time performance
/// filter). Selected via the comma-separated `osm_type` parameter.
struct query_kinds {
    bool nodes = true;
    bool ways = true;
    bool relations = true;
};

/// Mapnik vector datasource backed by an osmflat archive, queried by bounding
/// box through the `osmflat-capi` Rust shim.
class osmflat_datasource : public mapnik::datasource
{
public:
    explicit osmflat_datasource(mapnik::parameters const& params);
    virtual ~osmflat_datasource() = default;

    datasource_t type() const override;
    static const char* name();

    mapnik::featureset_ptr features(mapnik::query const& q) const override;
    mapnik::featureset_ptr features_at_point(mapnik::coord2d const& pt, double tol = 0) const override;

    mapnik::box2d<double> envelope() const override;
    std::optional<mapnik::datasource_geometry_t> get_geometry_type() const override;
    mapnik::layer_descriptor get_descriptor() const override;

private:
    void init(mapnik::parameters const& params);

    static const std::string name_;
    mapnik::layer_descriptor desc_;
    mapnik::box2d<double> extent_;
    query_kinds kinds_;

    // Tag prefilter from the `tags` param: (key, value); empty value == key=*.
    // Stored stably so the query can borrow the bytes.
    std::vector<std::pair<std::string, std::string>> tag_filters_;

    // Relation-membership filter from the `member_of` param, same encoding as
    // `tag_filters_` but the terms AND: nodes/ways are emitted only as members
    // of a relation matching all terms.
    std::vector<std::pair<std::string, std::string>> member_of_filters_;

    // Tag keys from the `numeric` param to coerce to numeric attributes.
    std::set<std::string> numeric_keys_;

    // Language preference from the `name_lang` param: an ordered list of
    // codes to try as "name:<code>" before falling back to the plain "name"
    // tag; "_" in the list means the plain "name" tag itself. Empty means no
    // substitution -- `name` resolves to the literal tag, as before.
    std::vector<std::string> name_langs_;

    // Draw order from the `order` param (default None = spatial order).
    OsmflatOrder order_ = OsmflatOrder::OsmflatOrder_None;

    // Precomputed once in init() for the per-query MAPNIK_LOG_DEBUG line in
    // features() -- the params don't change per query, so there's no reason
    // to re-stringify tag_filters_/member_of_filters_/kinds_ on every call.
    std::string log_kinds_;
    std::string log_tags_;
    std::string log_member_of_;

    // Simplification tolerance in pixels (the `simplify` param); 0 disables.
    // Converted to map units per query from the query resolution.
    double simplify_px_ = 0.5;

    std::shared_ptr<archive> archive_;

    // Correlation dump from the `dump` param (nullptr when unset). When set,
    // `features()` widens the fetched tag keys with a fixed naming/classifying
    // allowlist beyond what the active style references, so the dump carries
    // useful identity even for rules that only filter on e.g. `[highway]`.
    std::shared_ptr<dump_sink> dump_;

    // Compiled rules from the `group_hierarchy` param (nullptr when unset;
    // shared_ptr so it can be handed to each query's featureset without
    // copying the compiled expressions). Only meaningful alongside `dump_`:
    // the resolved group path/rank is dump-only, never exposed as a live
    // mapnik feature attribute, since nothing at render time needs it --
    // only the offline SVG post-processor does.
    std::shared_ptr<std::vector<group_rule>> group_rules_;
};

} // namespace osmflat

#endif // OSMFLAT_DATASOURCE_HPP
