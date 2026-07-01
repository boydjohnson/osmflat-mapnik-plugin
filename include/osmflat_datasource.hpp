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
#include <string>

#include "osmflat_archive.hpp"

namespace osmflat {

/// Which OSM primitive types the datasource emits.
enum class query_kind { nodes, ways, all };

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
    query_kind kind_;

    std::shared_ptr<archive> archive_;
};

} // namespace osmflat

#endif // OSMFLAT_DATASOURCE_HPP
