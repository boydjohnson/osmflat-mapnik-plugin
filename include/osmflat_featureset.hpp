#ifndef OSMFLAT_FEATURESET_HPP
#define OSMFLAT_FEATURESET_HPP

#include <mapnik/feature.hpp>
#include <mapnik/featureset.hpp>

#include <memory>
#include <string>
#include <vector>

#include "osmflat_archive.hpp"

namespace osmflat {

/// Mapnik featureset that pulls features one at a time out of an osmflat
/// `feature_set` query result, building mapnik geometries (points/lines) and
/// attaching attributes.
///
/// Because attributes are query-driven, the schema is *fixed* for the whole
/// query: the synthetic keys (`osm_id`, `osm_type`, `is_closed`) plus the
/// requested tag `keys`. So a single shared `context` is safe, and every
/// feature is completed against it — absent tags are filled with `value_null`,
/// which keeps style filters from throwing "Key does not exist".
class osmflat_featureset : public mapnik::Featureset
{
public:
    osmflat_featureset(feature_set&& fs, std::vector<std::string> keys);
    mapnik::feature_ptr next() override;

private:
    feature_set fs_;
    std::vector<std::string> keys_;   // requested tag names, aligned to attr(i)
    mapnik::context_ptr ctx_;
    mapnik::value_integer feature_id_ = 0;   // mapnik FID (unique within query)
};

} // namespace osmflat

#endif // OSMFLAT_FEATURESET_HPP
