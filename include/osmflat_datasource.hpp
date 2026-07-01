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

    // Tag keys from the `numeric` param to coerce to numeric attributes.
    std::set<std::string> numeric_keys_;

    // Draw order from the `order` param (default None = spatial order).
    OsmflatOrder order_ = OsmflatOrder::OsmflatOrder_None;

    // Simplification tolerance in pixels (the `simplify` param); 0 disables.
    // Converted to map units per query from the query resolution.
    double simplify_px_ = 0.5;

    std::shared_ptr<archive> archive_;
};

} // namespace osmflat

#endif // OSMFLAT_DATASOURCE_HPP
