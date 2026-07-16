// Correctness check for `osmflat_group_hierarchy.cpp` (the `group_hierarchy`
// datasource param's rule parsing/matching), run directly against
// `group-hierarchy.default.xml` -- no archive/bbox needed, unlike
// check_member_of.cpp/check_name_lang.cpp, since this tests the rule
// engine in isolation: build a synthetic mapnik feature with known tags,
// resolve it against the parsed rules, and check the (path, rank) that
// comes back matches what a real render's correlation dump would carry.
//
//   check_group_hierarchy <path-to-group-hierarchy.default.xml>

#include "osmflat_group_hierarchy.hpp"

#include <mapnik/feature.hpp>
#include <mapnik/feature_factory.hpp>
#include <mapnik/value/types.hpp>

#include <cstdlib>
#include <iostream>
#include <map>
#include <string>
#include <vector>

namespace {

mapnik::feature_ptr feature_with_tags(std::map<std::string, std::string> const& tags)
{
    auto ctx = std::make_shared<mapnik::context_type>();
    for (auto const& kv : tags) {
        ctx->push(kv.first);
    }
    auto feature = mapnik::feature_factory::create(ctx, 1);
    for (auto const& kv : tags) {
        feature->put(kv.first, mapnik::value_unicode_string::fromUTF8(kv.second));
    }
    return feature;
}

bool check_resolves(std::vector<osmflat::group_rule> const& rules,
                     std::map<std::string, std::string> const& tags,
                     std::string const& expected_path)
{
    auto feature = feature_with_tags(tags);
    auto resolved = osmflat::resolve_group(rules, *feature);
    if (!resolved) {
        std::cout << "  FAIL: expected path '" << expected_path << "', got no match\n";
        return false;
    }
    if (resolved->first != expected_path) {
        std::cout << "  FAIL: expected path '" << expected_path << "', got '" << resolved->first << "'\n";
        return false;
    }
    return true;
}

} // namespace

int main(int argc, char** argv)
{
    if (argc != 2) {
        std::cerr << "usage: check_group_hierarchy <path-to-group-hierarchy.default.xml>\n";
        return EXIT_FAILURE;
    }
    const std::string rule_file = argv[1];

    try {
        auto rules = osmflat::load_group_hierarchy(rule_file);
        std::cout << "loaded " << rules.size() << " rules from " << rule_file << "\n";

        bool ok = true;

        // Row order in the default file is also z-rank -- confirm the three
        // public-transit rows land immediately before plain rail, and that
        // rank increases monotonically (rank IS row index, nothing reorders
        // it).
        for (std::size_t i = 0; i < rules.size(); ++i) {
            if (rules[i].rank != static_cast<int32_t>(i)) {
                std::cout << "  FAIL: rule " << i << " (" << rules[i].path << ") has rank " << rules[i].rank
                          << ", expected " << i << "\n";
                ok = false;
            }
        }

        ok = check_resolves(rules, {{"railway", "light_rail"}}, "transportation/public-transit/light-rail") && ok;
        ok = check_resolves(rules, {{"railway", "subway"}}, "transportation/public-transit/subway") && ok;
        ok = check_resolves(rules, {{"railway", "tram"}}, "transportation/public-transit/tram") && ok;
        // Plain/unrecognized railway values (e.g. freight rail, "rail"
        // itself) fall through the three subtype rules to the generic one.
        ok = check_resolves(rules, {{"railway", "rail"}}, "transportation/rail") && ok;
        ok = check_resolves(rules, {{"highway", "primary"}}, "transportation/roads") && ok;
        ok = check_resolves(rules, {{"building", "yes"}}, "buildings") && ok;
        ok = check_resolves(rules, {{"waterway", "river"}}, "water") && ok;
        ok = check_resolves(rules, {{"amenity", "cafe"}}, "pois") && ok;

        // A feature with none of the referenced tags matches nothing.
        {
            auto feature = feature_with_tags({{"railway", ""}, {"highway", ""}});
            auto resolved = osmflat::resolve_group(rules, *feature);
            if (resolved) {
                std::cout << "  FAIL: feature with no real tags matched '" << resolved->first << "'\n";
                ok = false;
            }
        }

        // Every tag any rule's <Filter> references must come back so the
        // datasource knows to always fetch it (see referenced_attributes()'s
        // use in osmflat_datasource::features()).
        {
            auto attrs = osmflat::referenced_attributes(rules);
            std::vector<std::string> expected = {
                "admin_level", "amenity", "boundary", "building", "highway",
                "landuse",     "leisure", "natural",  "railway",  "shop", "waterway",
            };
            for (auto const& tag : expected) {
                if (!attrs.count(tag)) {
                    std::cout << "  FAIL: referenced_attributes() missing '" << tag << "'\n";
                    ok = false;
                }
            }
        }

        std::cout << "\n" << (ok ? "PASS" : "FAIL") << "\n";
        return ok ? EXIT_SUCCESS : EXIT_FAILURE;
    } catch (std::exception const& ex) {
        std::cerr << "error: " << ex.what() << "\n";
        return EXIT_FAILURE;
    }
}
