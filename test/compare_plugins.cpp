// Structural equivalence check: queries the same source data through the
// osmflat plugin and mapnik's geojson.input plugin, and compares the
// assembled polygon/multi_polygon features geometry-for-geometry.
//
// This compares ring counts and per-ring shoelace area (in degrees^2 -- a
// relative metric only, not true area) rather than diffing rendered pixels
// or raw point counts. Pixel diffs are sensitive to feature draw order
// (painter's algorithm) and rendering is not the plugin's contract; raw
// point counts are sensitive to closing-vertex duplication and to exactly
// where two independent ring-assemblers stitch a tolerance gap. Ring
// topology (how many disjoint polygons, how many holes each has) and area
// are stable across both of those and are what "the same shape" actually
// means here.
//
//   compare_plugins <osmflat_plugin_dir> <geojson_plugin_dir>
//                    <osmflat_archive_dir> <geojson_file>
//                    <minx> <miny> <maxx> <maxy> [area_tolerance_pct=1.0]

#include <mapnik/mapnik.hpp>
#include <mapnik/datasource_cache.hpp>
#include <mapnik/datasource.hpp>
#include <mapnik/featureset.hpp>
#include <mapnik/feature.hpp>
#include <mapnik/geometry.hpp>
#include <mapnik/geometry/box2d.hpp>
#include <mapnik/query.hpp>
#include <mapnik/params.hpp>
#include <mapnik/util/variant.hpp>

#include <algorithm>
#include <cmath>
#include <cstdlib>
#include <iomanip>
#include <iostream>
#include <map>
#include <string>
#include <vector>

namespace {

double ring_area(mapnik::geometry::linear_ring<double> const& ring)
{
    std::size_t n = ring.size();
    if (n < 3)
        return 0.0;
    double a = 0.0;
    for (std::size_t i = 0; i < n; ++i) {
        auto const& p0 = ring[i];
        auto const& p1 = ring[(i + 1) % n];
        a += p0.x * p1.y - p1.x * p0.y;
    }
    return std::abs(a) * 0.5;
}

std::size_t ring_unique_points(mapnik::geometry::linear_ring<double> const& ring)
{
    if (ring.size() > 1 && ring.front() == ring.back())
        return ring.size() - 1;
    return ring.size();
}

// Normalizes polygon and multi_polygon geometries to a flat polygon list;
// everything else (point, line_string, empty, ...) is "not an area feature".
struct polygons_visitor
{
    std::vector<mapnik::geometry::polygon<double>> operator()(mapnik::geometry::polygon<double> const& p) const
    {
        return {p};
    }
    std::vector<mapnik::geometry::polygon<double>> operator()(mapnik::geometry::multi_polygon<double> const& mp) const
    {
        return std::vector<mapnik::geometry::polygon<double>>(mp.begin(), mp.end());
    }
    template<typename T>
    std::vector<mapnik::geometry::polygon<double>> operator()(T const&) const
    {
        return {};
    }
};

struct PolygonSummary
{
    std::size_t ring_count = 0;
    std::vector<std::size_t> ring_points; // informational only, see file header
    double net_area = 0.0;                // exterior minus holes
};

struct FeatureSummary
{
    std::vector<PolygonSummary> polygons; // sorted by descending net_area
};

FeatureSummary summarize(mapnik::feature_ptr const& feat)
{
    auto polys = mapnik::util::apply_visitor(polygons_visitor{}, feat->get_geometry());
    FeatureSummary out;
    for (auto const& poly : polys) {
        PolygonSummary ps;
        ps.ring_count = poly.size();
        double area = 0.0;
        for (std::size_t r = 0; r < poly.size(); ++r) {
            ps.ring_points.push_back(ring_unique_points(poly[r]));
            double a = ring_area(poly[r]);
            area += (r == 0) ? a : -a; // ring 0 = exterior, rest = holes
        }
        ps.net_area = area;
        out.polygons.push_back(std::move(ps));
    }
    std::sort(out.polygons.begin(), out.polygons.end(), [](PolygonSummary const& a, PolygonSummary const& b) {
        return a.net_area > b.net_area;
    });
    return out;
}

std::string feature_key(mapnik::feature_ptr const& feat)
{
    std::string name = feat->has_key("name") ? feat->get("name").to_string() : std::string();
    std::string level = feat->has_key("admin_level") ? feat->get("admin_level").to_string() : std::string();
    if (name.empty())
        name = "<unnamed>";
    return name + "|admin_level=" + (level.empty() ? "?" : level);
}

std::map<std::string, FeatureSummary> collect(mapnik::datasource_ptr const& ds,
                                                mapnik::query const& q,
                                                std::size_t& total_seen)
{
    std::map<std::string, FeatureSummary> out;
    auto fs = ds->features(q);
    total_seen = 0;
    while (mapnik::feature_ptr feat = fs->next()) {
        ++total_seen;
        auto summary = summarize(feat);
        if (summary.polygons.empty())
            continue; // point/line_string/empty -- not an area feature
        out.emplace(feature_key(feat), std::move(summary));
    }
    return out;
}

} // namespace

int main(int argc, char** argv)
{
    if (argc < 9) {
        std::cerr << "usage: compare_plugins <osmflat_plugin_dir> <geojson_plugin_dir> "
                     "<osmflat_archive_dir> <geojson_file> "
                     "<minx> <miny> <maxx> <maxy> [area_tolerance_pct=1.0]\n";
        return EXIT_FAILURE;
    }
    const std::string osmflat_plugin_dir = argv[1];
    const std::string geojson_plugin_dir = argv[2];
    const std::string archive_dir = argv[3];
    const std::string geojson_file = argv[4];
    const double minx = std::stod(argv[5]);
    const double miny = std::stod(argv[6]);
    const double maxx = std::stod(argv[7]);
    const double maxy = std::stod(argv[8]);
    const double tol_pct = (argc > 9) ? std::stod(argv[9]) : 1.0;

    try {
        mapnik::setup();
        mapnik::datasource_cache::instance().register_datasources(osmflat_plugin_dir);
        mapnik::datasource_cache::instance().register_datasources(geojson_plugin_dir);

        mapnik::parameters osmflat_params;
        osmflat_params["type"] = std::string("osmflat");
        osmflat_params["file"] = archive_dir;
        osmflat_params["osm_type"] = std::string("relation");
        auto osmflat_ds = mapnik::datasource_cache::instance().create(osmflat_params);

        mapnik::parameters geojson_params;
        geojson_params["type"] = std::string("geojson");
        geojson_params["file"] = geojson_file;
        auto geojson_ds = mapnik::datasource_cache::instance().create(geojson_params);

        mapnik::box2d<double> bbox(minx, miny, maxx, maxy);
        mapnik::query q(bbox);
        // osmflat is query-driven: it only populates tags a query names (see
        // mapnik-gotchas memory). geojson.input always exposes every property
        // regardless, so naming these is a no-op there.
        q.add_property_name("name");
        q.add_property_name("admin_level");

        std::size_t osmflat_total = 0, geojson_total = 0;
        auto osmflat_feats = collect(osmflat_ds, q, osmflat_total);
        auto geojson_feats = collect(geojson_ds, q, geojson_total);

        std::cout << "osmflat: " << osmflat_total << " features queried, " << osmflat_feats.size()
                  << " area features\n";
        std::cout << "geojson: " << geojson_total << " features queried, " << geojson_feats.size()
                  << " area features\n\n";

        std::vector<std::string> keys;
        for (auto const& kv : osmflat_feats)
            keys.push_back(kv.first);
        for (auto const& kv : geojson_feats)
            if (!osmflat_feats.count(kv.first))
                keys.push_back(kv.first);
        std::sort(keys.begin(), keys.end());

        std::cout << std::left << std::setw(40) << "feature" << std::setw(10) << "osmflat" << std::setw(10)
                  << "geojson" << std::setw(14) << "max area d%"
                  << "verdict\n";

        bool ok = true;
        for (auto const& key : keys) {
            auto oi = osmflat_feats.find(key);
            auto gi = geojson_feats.find(key);
            if (oi == osmflat_feats.end() || gi == geojson_feats.end()) {
                std::cout << std::left << std::setw(40) << key << std::setw(10)
                          << (oi != osmflat_feats.end() ? std::to_string(oi->second.polygons.size()) : "-")
                          << std::setw(10)
                          << (gi != geojson_feats.end() ? std::to_string(gi->second.polygons.size()) : "-")
                          << std::setw(14) << "-"
                          << "MISSING\n";
                ok = false;
                continue;
            }
            auto const& op = oi->second.polygons;
            auto const& gp = gi->second.polygons;
            bool match = op.size() == gp.size();
            double max_diff_pct = 0.0;
            std::size_t n = std::min(op.size(), gp.size());
            for (std::size_t i = 0; i < n; ++i) {
                if (op[i].ring_count != gp[i].ring_count)
                    match = false;
                double a = op[i].net_area, b = gp[i].net_area;
                double denom = std::max(std::abs(a), std::abs(b));
                double diff_pct = denom > 0 ? std::abs(a - b) / denom * 100.0 : 0.0;
                max_diff_pct = std::max(max_diff_pct, diff_pct);
                if (diff_pct > tol_pct)
                    match = false;
            }
            std::cout << std::left << std::setw(40) << key << std::setw(10) << op.size() << std::setw(10)
                      << gp.size() << std::setw(14) << std::fixed << std::setprecision(4) << max_diff_pct
                      << (match ? "MATCH" : "MISMATCH") << "\n";
            if (!match)
                ok = false;
        }

        std::cout << "\n" << (ok ? "PASS" : "FAIL") << "\n";
        return ok ? EXIT_SUCCESS : EXIT_FAILURE;
    } catch (std::exception const& ex) {
        std::cerr << "error: " << ex.what() << "\n";
        return EXIT_FAILURE;
    }
}
