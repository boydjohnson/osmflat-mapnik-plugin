#ifndef OSMFLAT_FEATURESET_HPP
#define OSMFLAT_FEATURESET_HPP

#include <mapnik/feature.hpp>
#include <mapnik/featureset.hpp>

#include <memory>

#include "osmflat_archive.hpp"

namespace osmflat {

/// Mapnik featureset that pulls features one at a time out of an osmflat
/// `feature_set` query result, building mapnik geometries (points/lines) and
/// attaching tags as attributes.
///
/// Each feature gets its own `context` holding exactly that feature's keys
/// (`osm_id` + its tags). OSM tags are heterogeneous, so a single shared
/// context would claim keys that some features lack; a per-feature context
/// keeps the attribute set and the values perfectly aligned.
class osmflat_featureset : public mapnik::Featureset
{
public:
    explicit osmflat_featureset(feature_set&& fs);
    mapnik::feature_ptr next() override;

private:
    feature_set fs_;
};

} // namespace osmflat

#endif // OSMFLAT_FEATURESET_HPP
