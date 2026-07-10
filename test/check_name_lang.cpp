// Correctness check for the `name_lang` datasource param against
// test/fixtures/baarle-hertog.osm.flat. Ground truth below is read directly
// off test/fixtures/baarle-hertog.geojson's own name/name:fr/name:mk/name:nl/
// name:ru tags (an independent osmium export), not anything the plugin
// computes -- same convention as check_member_of.cpp.
//
// Baarle-Hertog / Baarle-Nassau is a good fixture for this: it's the only
// place in the archive tagged with alternate-language names, and its
// "name:nl" tag happens to equal its plain "name" tag for both places, which
// makes it a convenient *identity* key: every query below also requests
// "name:nl" alongside "name", so a relation's identity can be read off
// name:nl regardless of what `name_lang` does to "name" that query. (osm_id
// can't serve this role here -- assembled multipolygon relation features
// report no real OSM id; see osmflat_capi.hpp's `osmflat_feature_osm_id` doc
// comment.) Of the 4 admin_level rings in this bbox (see compare_plugins),
// only 3 carry translation tags at all -- Baarle-Nassau's admin_level=10
// ring has just a plain "name" -- so the identity filter below naturally
// excludes it.
//
//   check_name_lang <osmflat_plugin_dir> <archive_dir>
//                   <minx> <miny> <maxx> <maxy>

#include <mapnik/mapnik.hpp>
#include <mapnik/datasource_cache.hpp>
#include <mapnik/datasource.hpp>
#include <mapnik/featureset.hpp>
#include <mapnik/feature.hpp>
#include <mapnik/geometry/box2d.hpp>
#include <mapnik/query.hpp>
#include <mapnik/params.hpp>

#include <cstdlib>
#include <iostream>
#include <map>
#include <string>
#include <vector>

namespace {

// Ground truth from test/fixtures/baarle-hertog.geojson's own tags.
const std::map<std::string, std::string> kFrenchName = {
    {"Baarle-Hertog", "Baerle-Duc"},
    {"Baarle-Nassau", "Baerle-Nassau"},
};

struct Row {
    std::string identity;  // name:nl -- stable across name_lang variants
    std::string name;      // resolved "name" attribute for this query
};

std::vector<Row> query_rows(mapnik::datasource_ptr const& ds, mapnik::box2d<double> const& bbox)
{
    mapnik::query q(bbox);
    q.add_property_name("name");
    q.add_property_name("name:nl");
    auto fs = ds->features(q);
    std::vector<Row> out;
    while (mapnik::feature_ptr feat = fs->next()) {
        Row row;
        mapnik::value const& identity = feat->get("name:nl");
        row.identity = identity.is_null() ? std::string() : identity.to_string();
        mapnik::value const& name = feat->get("name");
        row.name = name.is_null() ? std::string() : name.to_string();
        out.push_back(std::move(row));
    }
    return out;
}

// Rows whose identity is one of the two known Baarle-* places.
std::vector<Row> baarle_rows(std::vector<Row> const& rows)
{
    std::vector<Row> out;
    for (auto const& r : rows) {
        if (kFrenchName.count(r.identity)) {
            out.push_back(r);
        }
    }
    return out;
}

mapnik::datasource_ptr open_ds(std::string const& archive_dir, std::string const& name_lang)
{
    mapnik::parameters params;
    params["type"] = std::string("osmflat");
    params["file"] = archive_dir;
    params["osm_type"] = std::string("relation");
    if (!name_lang.empty()) {
        params["name_lang"] = name_lang;
    }
    return mapnik::datasource_cache::instance().create(params);
}

} // namespace

int main(int argc, char** argv)
{
    if (argc != 7) {
        std::cerr << "usage: check_name_lang <osmflat_plugin_dir> <archive_dir> "
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

        // 1. Baseline (no name_lang): establishes there are exactly 3
        // Baarle-* admin boundaries carrying translation tags -- of the 4
        // admin_level rings compare_plugins finds in this bbox, only 3 have
        // name:fr/name:nl/etc; "Baarle-Nassau|admin_level=10" has just a
        // plain "name" and is correctly excluded by the name:nl identity
        // filter -- and that their plain "name" already matches their
        // identity (name:nl), which the later cases lean on.
        auto baseline = baarle_rows(query_rows(open_ds(archive_dir, ""), bbox));
        std::cout << "baseline: " << baseline.size() << " Baarle-* boundaries (expected 3)\n";
        if (baseline.size() != 3) {
            std::cout << "  FAIL: expected exactly 3 Baarle-Hertog/Baarle-Nassau admin boundaries with translations\n";
            ok = false;
        }
        for (auto const& r : baseline) {
            if (r.name != r.identity) {
                std::cout << "  FAIL: baseline name '" << r.name << "' != identity '" << r.identity << "'\n";
                ok = false;
            }
        }

        // 2. name_lang=fr: every known Baarle-* boundary must resolve to its
        // French name tag.
        {
            auto rows = baarle_rows(query_rows(open_ds(archive_dir, "fr"), bbox));
            bool all_match = rows.size() == 3;
            for (auto const& r : rows) {
                if (r.name != kFrenchName.at(r.identity)) {
                    std::cout << "  FAIL: " << r.identity << ": name_lang=fr got '" << r.name << "', expected '"
                              << kFrenchName.at(r.identity) << "'\n";
                    all_match = false;
                }
            }
            std::cout << "name_lang=fr: " << (all_match ? "all Baarle-* boundaries translated" : "MISMATCH") << "\n";
            ok = ok && all_match;
        }

        // 3. name_lang=xx,fr: "xx" isn't a real language tag on anything in
        // this fixture, so this must skip it and fall through to fr -- same
        // result as case 2. Proves priority-order fallthrough past a token
        // with no matching tag.
        {
            auto rows = baarle_rows(query_rows(open_ds(archive_dir, "xx,fr"), bbox));
            bool all_match = rows.size() == 3;
            for (auto const& r : rows) {
                if (r.name != kFrenchName.at(r.identity)) {
                    all_match = false;
                }
            }
            std::cout << "name_lang=xx,fr: " << (all_match ? "falls through to fr as expected" : "MISMATCH") << "\n";
            ok = ok && all_match;
        }

        // 4. name_lang=xx (no fallback token): must resolve to null, NOT
        // silently keep the plain name. This is the coalesce-fidelity case --
        // mod_tile's PostGIS rewrite only falls back to the plain tag when
        // the caller explicitly appends "_"; without it, an unmatched
        // language is null, same as SQL `coalesce(tags->'name:xx')`.
        {
            auto rows = baarle_rows(query_rows(open_ds(archive_dir, "xx"), bbox));
            bool all_null = rows.size() == 3;
            for (auto const& r : rows) {
                if (!r.name.empty()) {
                    std::cout << "  FAIL: " << r.identity << ": name_lang=xx (no fallback) got '" << r.name
                              << "', expected null\n";
                    all_null = false;
                }
            }
            std::cout << "name_lang=xx (no _): " << (all_null ? "resolves to null as expected" : "MISMATCH") << "\n";
            ok = ok && all_null;
        }

        // 5. name_lang=xx,_: same missing "xx", but with an explicit "_"
        // fallback token this time -- must recover the plain tag.
        {
            auto rows = baarle_rows(query_rows(open_ds(archive_dir, "xx,_"), bbox));
            bool all_match = rows.size() == 3;
            for (auto const& r : rows) {
                if (r.name != r.identity) {
                    std::cout << "  FAIL: " << r.identity << ": name_lang=xx,_ got '" << r.name
                              << "', expected plain name '" << r.identity << "'\n";
                    all_match = false;
                }
            }
            std::cout << "name_lang=xx,_: " << (all_match ? "falls back to plain name as expected" : "MISMATCH") << "\n";
            ok = ok && all_match;
        }

        std::cout << "\n" << (ok ? "PASS" : "FAIL") << "\n";
        return ok ? EXIT_SUCCESS : EXIT_FAILURE;
    } catch (std::exception const& ex) {
        std::cerr << "error: " << ex.what() << "\n";
        return EXIT_FAILURE;
    }
}
