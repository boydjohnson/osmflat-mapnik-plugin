// Correctness check for the `member_of` datasource param against
// test/fixtures/bicycle-route.osm.flat (relation 6573, ref 09-33, 11 way
// members; see fetch-bicycle-route.sh). Ground truth below is the member way
// id list read directly off the Overpass fetch, independent of anything the
// plugin computes.
//
// There's no independent-plugin cross-check here the way compare_plugins.cpp
// uses geojson.input for multipolygons: geojson has no relation-membership
// concept to compare against. So this instead checks the plugin's own
// contract directly: the filter must return exactly the known members (not a
// subset, not extra ways), tag every one with the parent relation's rel_ref
// via the forward join, and return nothing for a filter that matches no
// relation -- the fixture's 263 non-member highway ways are the negative
// control that makes that last check meaningful.
//
//   check_member_of <osmflat_plugin_dir> <archive_dir>
//                    <minx> <miny> <maxx> <maxy>

#include <mapnik/mapnik.hpp>
#include <mapnik/datasource_cache.hpp>
#include <mapnik/datasource.hpp>
#include <mapnik/featureset.hpp>
#include <mapnik/feature.hpp>
#include <mapnik/geometry/box2d.hpp>
#include <mapnik/query.hpp>
#include <mapnik/params.hpp>

#include <algorithm>
#include <cstdlib>
#include <iostream>
#include <set>
#include <string>
#include <vector>

namespace {

// Relation 6573 (route=bicycle, ref=09-33) way members, from the Overpass
// fetch in fetch-bicycle-route.sh -- ground truth, not plugin output.
const std::set<long long> kExpectedMemberIds = {23176634, 23176673, 192343755, 23176672, 38241137,
                                                 23176705,  26528058, 23176796,  38240909, 23176951,
                                                 38240906};
constexpr std::size_t kExpectedTotalWays = 274;

struct WayHit
{
    long long osm_id;
    std::string rel_ref;
};

std::vector<WayHit> query_ways(mapnik::datasource_ptr const& ds, mapnik::box2d<double> const& bbox)
{
    mapnik::query q(bbox);
    q.add_property_name("rel_ref");
    auto fs = ds->features(q);
    std::vector<WayHit> out;
    while (mapnik::feature_ptr feat = fs->next()) {
        WayHit hit;
        hit.osm_id = static_cast<long long>(feat->get("osm_id").to_double());
        hit.rel_ref = feat->has_key("rel_ref") ? feat->get("rel_ref").to_string() : std::string();
        out.push_back(hit);
    }
    return out;
}

mapnik::datasource_ptr open_ds(std::string const& archive_dir, std::string const& member_of)
{
    mapnik::parameters params;
    params["type"] = std::string("osmflat");
    params["file"] = archive_dir;
    params["osm_type"] = std::string("way");
    if (!member_of.empty())
        params["member_of"] = member_of;
    return mapnik::datasource_cache::instance().create(params);
}

} // namespace

int main(int argc, char** argv)
{
    if (argc != 7) {
        std::cerr << "usage: check_member_of <osmflat_plugin_dir> <archive_dir> "
                     "<minx> <miny> <maxx> <maxy>\n";
        return EXIT_FAILURE;
    }
    const std::string plugin_dir = argv[1];
    const std::string archive_dir = argv[2];
    const double minx = std::stod(argv[3]);
    const double miny = std::stod(argv[4]);
    const double maxx = std::stod(argv[5]);
    const double maxy = std::stod(argv[6]);

    try {
        mapnik::setup();
        mapnik::datasource_cache::instance().register_datasources(plugin_dir);
        mapnik::box2d<double> bbox(minx, miny, maxx, maxy);

        bool ok = true;

        // 1. No member_of: every way in the archive should come back.
        {
            auto ds = open_ds(archive_dir, "");
            auto hits = query_ways(ds, bbox);
            std::cout << "no filter:        " << hits.size() << " ways (expected " << kExpectedTotalWays
                      << ")\n";
            if (hits.size() != kExpectedTotalWays) {
                std::cout << "  FAIL: total way count doesn't match the archive's actual way count\n";
                ok = false;
            }
        }

        // 2. member_of=route=bicycle,ref=09-33: exactly the 11 known members,
        //    each carrying the parent relation's ref via rel_ref.
        {
            auto ds = open_ds(archive_dir, "route=bicycle,ref=09-33");
            auto hits = query_ways(ds, bbox);
            std::set<long long> got;
            bool all_tagged = true;
            for (auto const& h : hits) {
                got.insert(h.osm_id);
                if (h.rel_ref != "09-33")
                    all_tagged = false;
            }
            std::cout << "member_of=09-33:  " << hits.size() << " ways (expected " << kExpectedMemberIds.size()
                      << ")\n";
            if (got != kExpectedMemberIds) {
                std::cout << "  FAIL: returned way ids don't match the known member set\n";
                std::vector<long long> missing, extra;
                std::set_difference(kExpectedMemberIds.begin(), kExpectedMemberIds.end(), got.begin(), got.end(),
                                     std::back_inserter(missing));
                std::set_difference(got.begin(), got.end(), kExpectedMemberIds.begin(), kExpectedMemberIds.end(),
                                     std::back_inserter(extra));
                for (auto id : missing)
                    std::cout << "    missing member way " << id << "\n";
                for (auto id : extra)
                    std::cout << "    unexpected extra way " << id << "\n";
                ok = false;
            }
            if (!all_tagged) {
                std::cout << "  FAIL: not every returned way carries rel_ref=09-33\n";
                ok = false;
            }
        }

        // 3. member_of matching no relation: must return nothing, not
        //    everything -- the actual bug this filter could plausibly have.
        {
            auto ds = open_ds(archive_dir, "route=bicycle,ref=99-99");
            auto hits = query_ways(ds, bbox);
            std::cout << "member_of=99-99:  " << hits.size() << " ways (expected 0)\n";
            if (!hits.empty()) {
                std::cout << "  FAIL: an unmatched member_of filter returned ways instead of none\n";
                ok = false;
            }
        }

        std::cout << "\n" << (ok ? "PASS" : "FAIL") << "\n";
        return ok ? EXIT_SUCCESS : EXIT_FAILURE;
    } catch (std::exception const& ex) {
        std::cerr << "error: " << ex.what() << "\n";
        return EXIT_FAILURE;
    }
}
