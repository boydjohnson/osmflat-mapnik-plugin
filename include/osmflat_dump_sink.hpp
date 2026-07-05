#ifndef OSMFLAT_DUMP_SINK_HPP
#define OSMFLAT_DUMP_SINK_HPP

#include <cstdint>
#include <fstream>
#include <mutex>
#include <string>
#include <vector>

namespace osmflat {

/// One correlation record: the identity and raw (pre-projection, EPSG:4326)
/// geometry of a feature exactly as handed to mapnik, so an offline
/// post-processor can match it back to the SVG path(s) mapnik eventually
/// renders for it.
///
/// Geometry mirrors GeoJSON nesting: Point coordinates are a single (lon, lat)
/// pair; LineString is a flat vertex ring; MultiPolygon is polygons of rings
/// (ring 0 = exterior, rest = holes). Ways are always dumped as LineString
/// (mapnik treats closed ways as rings, not polygons — see `is_closed`), so
/// only those two shapes plus MultiPolygon (assembled relations) occur.
struct dump_record {
    std::string osm_type;      // "node" | "way" | "relation"
    bool has_osm_id = false;
    uint64_t osm_id = 0;
    int32_t z_order = 0;
    double way_area = 0.0;
    bool is_closed = false;

    std::string geom_type;     // "Point" | "LineString" | "MultiPolygon"
    std::vector<std::pair<double, double>> ring;                       // Point/LineString
    std::vector<std::vector<std::vector<std::pair<double, double>>>> polygons; // MultiPolygon

    std::vector<std::pair<std::string, std::string>> tags;   // present tags only
};

/// Appends correlation records as newline-delimited GeoJSON Features. Opens in
/// append mode so several datasource instances (e.g. multiple `<Layer>`s all
/// pointing `dump` at the same path) can share one file without truncating
/// each other — callers are responsible for clearing the file before a fresh
/// render run. Writes are mutex-guarded for safety if a process ever renders
/// concurrently.
class dump_sink {
public:
    explicit dump_sink(const std::string& path);

    void write(const dump_record& rec);

private:
    std::mutex mutex_;
    std::ofstream out_;
};

} // namespace osmflat

#endif // OSMFLAT_DUMP_SINK_HPP
