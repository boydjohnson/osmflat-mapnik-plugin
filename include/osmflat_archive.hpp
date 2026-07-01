#ifndef OSMFLAT_ARCHIVE_HPP
#define OSMFLAT_ARCHIVE_HPP

#include <array>
#include <cstdint>
#include <stdexcept>
#include <string>
#include <vector>

#include "osmflat_capi.hpp"

namespace osmflat {

/// RAII owner of a query result. Wraps the opaque `OsmflatFeatureSet*` and
/// exposes the pull-based cursor (`next()` + per-feature getters). Pointers
/// returned by the getters are valid only until the following `next()`.
class feature_set {
public:
    explicit feature_set(OsmflatFeatureSet* fs) : fs_(fs) {}
    ~feature_set() { if (fs_) { osmflat_featureset_free(fs_); } }

    feature_set(const feature_set&) = delete;
    feature_set& operator=(const feature_set&) = delete;
    feature_set(feature_set&& o) noexcept : fs_(o.fs_) { o.fs_ = nullptr; }
    feature_set& operator=(feature_set&& o) noexcept {
        if (this != &o) {
            if (fs_) { osmflat_featureset_free(fs_); }
            fs_ = o.fs_;
            o.fs_ = nullptr;
        }
        return *this;
    }

    /// Advance to the next feature; false when exhausted.
    bool next() { return fs_ && osmflat_featureset_next(fs_); }

    /// Real OSM id of the current feature; false if the archive has none.
    bool osm_id(uint64_t& out) const { return osmflat_feature_osm_id(fs_, &out); }
    OsmflatOsmType osm_type() const { return osmflat_feature_osm_type(fs_); }
    bool is_closed() const { return osmflat_feature_is_closed(fs_); }
    OsmflatGeomType geom_type() const { return osmflat_feature_geom_type(fs_); }

    std::size_t num_coords() const { return osmflat_feature_num_coords(fs_); }
    const double* coords() const { return osmflat_feature_coords(fs_); }

    // MultiPolygon geometry (geom_type == MultiPolygon).
    std::size_t num_polygons() const { return osmflat_feature_num_polygons(fs_); }
    std::size_t polygon_num_rings(std::size_t p) const {
        return osmflat_feature_polygon_num_rings(fs_, p);
    }
    std::size_t ring_num_coords(std::size_t p, std::size_t r) const {
        return osmflat_feature_ring_num_coords(fs_, p, r);
    }
    const double* ring_coords(std::size_t p, std::size_t r) const {
        return osmflat_feature_ring_coords(fs_, p, r);
    }

    /// Value of the `i`th requested attribute (aligned to the query's keys).
    /// Returns false when the tag is absent (→ render as null).
    bool attr(std::size_t i, std::string& out) const {
        OsmflatValue v = osmflat_feature_attr(fs_, i);
        if (!v.present) { return false; }
        out.assign(reinterpret_cast<const char*>(v.ptr), v.len);
        return true;
    }

private:
    OsmflatFeatureSet* fs_;
};

/// RAII owner of an opened osmflat archive (the opaque `OsmflatArchive*`).
class archive {
public:
    explicit archive(const std::string& path) {
        handle_ = osmflat_archive_open(path.c_str());
        if (!handle_) {
            throw std::runtime_error("osmflat: failed to open archive at '" + path + "'");
        }
    }
    ~archive() { if (handle_) { osmflat_archive_free(handle_); } }

    archive(const archive&) = delete;
    archive& operator=(const archive&) = delete;

    /// Bounding box of the archive in degrees, as (min_x, min_y, max_x, max_y).
    std::array<double, 4> envelope() const {
        std::array<double, 4> e{};
        if (!osmflat_archive_envelope(handle_, &e[0], &e[1], &e[2], &e[3])) {
            throw std::runtime_error("osmflat: failed to read archive envelope");
        }
        return e;
    }

    /// Bounding-box query. `keys` are the tag names to materialize (borrowed;
    /// their bytes must outlive the call). Returns an owning `feature_set`.
    feature_set query(double min_x, double min_y, double max_x, double max_y,
                      bool include_nodes, bool include_ways, bool include_relations,
                      const std::vector<OsmflatStrRef>& keys) const {
        return feature_set(osmflat_query(handle_, min_x, min_y, max_x, max_y,
                                         include_nodes, include_ways, include_relations,
                                         keys.data(), keys.size()));
    }

private:
    OsmflatArchive* handle_ = nullptr;
};

} // namespace osmflat

#endif // OSMFLAT_ARCHIVE_HPP
