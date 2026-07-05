#!/usr/bin/env bash
# Regenerates test/fixtures/bicycle-route.osm.pbf: a small Dutch "knooppunt"
# (node-network) bicycle route relation -- relation 6573, ref 09-33, 11 way
# members -- plus ~270 nearby highway ways that are NOT route members. The
# non-members are a deliberate negative control: they exercise whether the
# `member_of` datasource param correctly *excludes* ways as well as includes
# them, which a fixture containing only the route's own members couldn't
# test (everything would trivially match).
#
# Also regenerates the derived fixtures checked in alongside the PBF:
#   bicycle-route.geojson    - osmium export (informational only: osmium has
#                              no route-relation assembly, so this is just
#                              the loose way/node soup, not comparable to
#                              osmflat's member_of-filtered output the way
#                              baarle-hertog.geojson is for multipolygons)
#   bicycle-route.osm.flat/  - osmflatc archive
#   bicycle-route.osm.ext/   - osmflat-extc --taginfo sidecar
#
# Overpass's main endpoint (overpass-api.de) was returning 406 for every
# query when this was written; the z.overpass-api.de mirror worked. If this
# script gets 406s, try swapping the host.
#
# Requires: curl, osmium (brew install osmium-tool), and sibling checkouts of
# osmflat-rs (../../../osmflat-rs) and osmflat-ext (../../../osmflat-ext)
set -euo pipefail
cd "$(dirname "$0")"

# Note the `(._; >;); out meta;` idiom rather than baarle-hertog's
# `out body; >; out skel qt;`: here the top-level query has two overlapping
# sets (the relation, and an independent highway/bbox match) whose downward
# closures intersect -- 5 ways are both route members and tagged highway.
# Emitting them via two separate `out` calls duplicated those 5 ways in the
# file, which `osmium sort` then rejected ("Way ID twice in input"). Unioning
# the set with its own recursive closure *before* the single `out` dedupes it
# (Overpass sets are proper sets). baarle-hertog's query has only one
# top-level set, so it never hit this.
curl -s --max-time 90 --data-urlencode 'data@-' \
    https://z.overpass-api.de/api/interpreter -o bicycle-route.osm <<'EOF'
[out:xml][timeout:90];
(
  rel(6573);
  way(51.363,4.673,51.421,4.722)["highway"];
);
(._; >;);
out meta;
EOF

# osmium extract needs nodes/ways/relations in ID order; Overpass emits the
# relation first, so sort before extracting. --set-bounds writes a real
# header bbox -- without it, osmflat_archive_envelope() reads (0,0,0,0) from
# the PBF header and every render/query comes back silently empty (see
# mapnik-gotchas memory).
osmium sort bicycle-route.osm -o bicycle-route-sorted.osm.pbf --overwrite
osmium extract \
    --bbox 4.6420262,51.3556074,4.735026,51.4269586 \
    --set-bounds -s complete_ways --overwrite \
    -o bicycle-route.osm.pbf bicycle-route-sorted.osm.pbf

rm -f bicycle-route.osm bicycle-route-sorted.osm.pbf
osmium fileinfo bicycle-route.osm.pbf

osmium export bicycle-route.osm.pbf -n -o bicycle-route.geojson -f geojson --overwrite

# --ids: the member_of correctness check identifies features by original OSM
# way id (to compare against a hardcoded ground-truth member list), which
# needs the optional ids sub-archive.
rm -rf bicycle-route.osm.flat
cargo run --release --manifest-path ../../../osmflat-rs/osmflatc/Cargo.toml -- \
    bicycle-route.osm.pbf bicycle-route.osm.flat --ids

rm -rf bicycle-route.osm.ext
cargo run --release --manifest-path ../../../osmflat-ext/osmflat-extc/Cargo.toml -- \
    --taginfo --out bicycle-route.osm.ext bicycle-route.osm.flat
