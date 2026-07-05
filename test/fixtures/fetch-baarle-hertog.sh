#!/usr/bin/env bash
# Regenerates test/fixtures/baarle-hertog.osm.pbf: the Baarle-Hertog (Belgium)
# / Baarle-Nassau (Netherlands) enclave complex. Baarle-Hertog is a single
# `boundary` relation whose "outer" members form ~20+ disjoint closed rings
# (scattered exclaves), not one ring with holes — a real-world torture test
# for ring assembly beyond what simple multipolygon-with-holes fixtures cover.
#
# Requires: curl, osmium (brew install osmium-tool)
set -euo pipefail
cd "$(dirname "$0")"

curl -s --max-time 60 --data-urlencode 'data@-' \
    https://overpass-api.de/api/interpreter -o baarle.osm <<'EOF'
[out:xml][timeout:60];
rel["boundary"="administrative"]["name"~"Baarle"];
out body;
>;
out skel qt;
EOF

# osmium extract needs nodes/ways/relations in ID order; Overpass emits
# relations first, so sort before extracting. --set-bounds writes a real
# header bbox — without it, osmflat_archive_envelope() reads (0,0,0,0) from
# the PBF header and every render comes back silently blank (see
# mapnik-gotchas memory).
osmium sort baarle.osm -o baarle-sorted.osm.pbf --overwrite
osmium extract \
    --bbox 4.7665604,51.392824,5.0109964,51.4809296 \
    --set-bounds -s complete_ways --overwrite \
    -o baarle-hertog.osm.pbf baarle-sorted.osm.pbf

rm -f baarle.osm baarle-sorted.osm.pbf
osmium fileinfo baarle-hertog.osm.pbf
