# osmflat-mapnik-plugin

A [mapnik](https://mapnik.org) vector datasource plugin that reads
[osmflat](https://github.com/boydjohnson/osmflat-rs) archives and answers
bounding-box queries using the archive's spatial index.

## Architecture

```
mapnik  ──loads──▶  osmflat.input  (C++ MODULE)
                         │  src/osmflat_datasource.cpp   (mapnik datasource)
                         │  src/osmflat_featureset.cpp   (pull-based featureset)
                         │  include/osmflat_archive.hpp  (RAII over the C API)
                         ▼
                    libosmflat_capi.a  (Rust staticlib, baked in)
                         │  rust/osmflat-capi/src/lib.rs (#[no_mangle] C ABI)
                         │  build.rs → cbindgen → include/osmflat_capi.hpp
                         ▼
                    osmflat crate  (github.com/boydjohnson/osmflat-rs, main)
                         find_nodes/ways_by_bounding_box, iter_tags
```

The Rust shim is linked as a **staticlib**, so the whole plugin ships as a
single `osmflat.input` file with no separate dylib to locate at runtime.

## Build (one step)

[Corrosion](https://github.com/corrosion-rs/corrosion) makes CMake the single
driver: one `cmake --build` compiles the Rust staticlib (running `cbindgen` via
`build.rs` to regenerate the header), then builds and links the C++ module.

```sh
cmake -S . -B build -DCMAKE_BUILD_TYPE=Release
cmake --build build
# → build/plugins/osmflat.input
```

Requirements: a C++17 compiler, CMake ≥ 3.22, Rust/Cargo, **mapnik 4**
(`libmapnik` discoverable via `pkg-config`), and Boost headers. On
macOS/Homebrew the Boost include path is added automatically via
`brew --prefix`.

Mapnik 3.x won't work: `mapnik::parameters::get` returns `std::optional` in 4.x
and `boost::optional` in 3.x, which this plugin's `init()` relies on
throughout, so CMake requires `libmapnik>=4.0` rather than letting the build
fail later with template errors. In practice that means Homebrew (4.2+) or
Ubuntu 26.04 LTS (4.2.1); 24.04 and 22.04 still ship mapnik 3.1, so building
there means building mapnik from source.

`osmflat` and `osmflat-ext` are git dependencies (branch `main`) of the Rust
crate, so a clone of this repo builds on its own — no sibling checkouts — but
the first build needs network access for cargo to fetch them.

## Static, self-contained `render`

`-DOSMFLAT_STATIC_RENDER=ON` builds mapnik from source (v4.2.2 by default) as a
**static** library with the osmflat datasource compiled in as a built-in
plugin, and links `render` against it. The result loads nothing at runtime — no
`libmapnik`, no `osmflat.input`, no system fonts — so it can be copied to a
machine that has never heard of mapnik:

```sh
cmake -S . -B build-static -DCMAKE_BUILD_TYPE=Release \
    -DOSMFLAT_STATIC_RENDER=ON -DOSMFLAT_FULLY_STATIC=ON
cmake --build build-static --target render
```

`OSMFLAT_FULLY_STATIC=ON` adds `-static` (musl only: libc goes in too). The
`<plugin_dir>` argument stays in the CLI but is ignored — loading a `.input`
module would pull in a second, shared mapnik.

Mapnik's static-plugin table is compile-time, so
`cmake/static-mapnik/patch-mapnik.cmake` edits the fetched mapnik tree: it adds
`plugins/input/osmflat`, links it into `libmapnik`, registers it in
`datasource_cache_static.cpp`, and teaches `projection` to accept
`+proj=longlat ...` (see below). Every edit anchors on an exact upstream string
and fails loudly if mapnik moved it.

Trimmed to what `render` needs: AGG + PNG, freetype/harfbuzz/ICU for text. No
cairo, PROJ, grid/SVG renderers, or stock input plugins. Dropping PROJ is what
keeps the binary free of a 9 MB `proj.db`, but mapnik then only knows
`epsg:4326` / `epsg:3857` by name — and, more subtly, classifies `epsg:4326` as
*geographic*, which scales `scale_denominator` by ~111319 versus what PROJ
reports for the equivalent `+proj=longlat +datum=WGS84 +no_defs`. Since every
style here is tuned against the degree-based numbers (`0.1` ≈ neighborhood),
the patch makes a PROJ-less mapnik classify `+proj=longlat` exactly as PROJ
does, so styles render identically either way. Other proj4 strings still throw.

Fonts: `$MAPNIK_FONT_DIR` if set, else `fonts/` beside the binary (which is how
the release tarball ships DejaVu).

`scripts/build-static-render.sh` does the whole thing in Alpine — installs the
static dependency archives, builds, runs ctest, and packages
`dist/osmflat-render-<version>-<arch>-linux-musl.tar.gz`:

```sh
podman run --rm -v "$PWD:/src" -w /src alpine:3.22 sh scripts/build-static-render.sh
```

`.github/workflows/static-render.yml` runs that same script under `docker run`
for x86_64 and aarch64 and uploads both tarballs, caching the build tree
(mapnik is ~16 min cold, ~1 min warm). `.github/workflows/release.yml` builds
the same two archives from a clean tree on a `v*` tag — the tag has to match
`project(... VERSION)` — smoke-tests each unpacked archive on a bare alpine
image, and publishes them with a `SHA256SUMS`. `workflow_dispatch` on that
workflow builds without publishing, to check an arch before cutting the tag.

Each archive carries `render`, `fonts/`, mapnik's `LICENSE.mapnik`, and a
`NOTICE` naming every statically linked library and how to relink.

**Licensing:** mapnik is LGPL-2.1, so a distributed binary with mapnik linked
in must let recipients relink it against a modified mapnik — publishing these
sources plus the build scripts is what covers that.

## Datasource parameters

| param      | required | values              | meaning                          |
|------------|----------|---------------------|----------------------------------|
| `type`     | yes      | `osmflat`           | selects this plugin              |
| `file`     | yes      | path                | the `*.osm.flat` archive dir     |
| `osm_type` | no       | comma-separated `node`\|`way`\|`relation`\|`all` | primitives to emit (default all); e.g. `way,relation` |
| `ext`      | no       | path                | Ext sidecar dir (`*.osm.ext`) enabling tag push-down and, if built with `--multipolygons`, precomputed relation ring assembly |
| `tags`     | no       | comma-separated `key=value` / `key=*` | tag prefilter, e.g. `highway=*` or `natural=water,natural=wood` |
| `member_of` | no      | comma-separated `key=value` / `key=*` | relation-membership filter: emit only nodes/ways that are members of a relation matching **all** terms, e.g. `route=train,ref=Borealis` |
| `numeric`  | no       | comma-separated keys | expose these tags as numbers so `[lanes] > 2` compares numerically |
| `order`    | no       | `z_order`\|`way_area`\|`none` | draw order of returned features (default spatial); `z_order` for roads, `way_area` (desc) for areas |
| `simplify` | no       | pixels (float, default `0.5`) | scale-aware Douglas–Peucker tolerance; geometry generalized to sub-pixel per zoom. `0` disables |
| `dump`     | no       | path                | append-mode NDJSON correlation dump; see below |

The datasource is **semantically neutral**: nodes → points, ways → line strings
(open *and* closed alike — no area heuristics). The style decides fill vs. stroke
per tag (mapnik's `PolygonSymbolizer` fills a closed ring, `LineSymbolizer`
strokes it). `type=multipolygon`/`boundary` **relations** are assembled from their
member ways into `multi_polygon` geometry (outer rings + holes).

**Attributes are query-driven:** each feature exposes exactly the tags the active
style references (`[highway]`, `[natural]`, …), null-filled when absent, plus
synthetic facts: `osm_id` (Integer, real OSM id), `osm_type`
(String: node/way/relation), `is_closed` (Boolean), `way_area` (Double,
enclosed area in spherical m² for closed ways / multipolygons; 0 otherwise —
e.g. `[way_area] > 1000000` for areas over 1 km²), and `z_order` (Integer,
osm2pgsql-style render priority = `layer*10000 + bridge/tunnel band + highway
class rank`; also drives the `order=z_order` sort).

## Running the tests

Every check below is registered with ctest (under `-DBUILD_RENDER_TEST=ON`),
against the checked-in fixtures, so the whole suite is one command:

```sh
cmake -S . -B build -DCMAKE_BUILD_TYPE=Release -DBUILD_RENDER_TEST=ON
cmake --build build
ctest --test-dir build --output-on-failure   # C++ side
(cd rust/osmflat-capi && cargo test)         # Rust C-ABI side
```

That is exactly what `.github/workflows/ci.yml` runs on every pull request,
inside an `ubuntu:26.04` container so CI builds against the mapnik an Ubuntu
LTS user gets from `apt install libmapnik-dev`. The individual harnesses are
documented below — run them by hand (with other archives, bboxes or styles)
when a fixture-sized case isn't what you need.

## Smoke test

Loads the plugin through mapnik's `datasource_cache`, applies `test/style.xml`
(base gray ways + red `[highway]='primary'`), and renders a PNG:

```sh
cmake -S . -B build -DBUILD_RENDER_TEST=ON
cmake --build build
./build/render ./build/plugins <style.xml> <archive_dir> out.png \
    <minx> <miny> <maxx> <maxy>
# e.g. Mexico City:
./build/render ./build/plugins ./test/style.xml \
    ../osmflat-rs/mexico.osm.flat mexico.png -99.30 19.20 -98.95 19.60
```

Example styles under `test/` (each uses `@ARCHIVE@` as the archive placeholder):

| style | shows |
|-------|-------|
| `style.xml`         | base ways + red `[highway]='primary'` filter |
| `style-full.xml`    | full multi-scale basemap: landcover+relation fills, buildings, road ramp w/ bridges+arrows, area/street/POI labels, city/town place labels, `MaxScaleDenominator` gating from neighborhood to state/country |
| `style-labels.xml`  | line-placement street labels + POI labels (needs fonts) |
| `style-streets.xml` | urban street ramp: casing/fill tiers, oneway arrows, bridges, labels |
| `style-relations.xml` | `type=multipolygon`/`boundary` relations as filled polygons with holes |
| `style-borealis.xml` | `member_of` route highlighting: only the Amtrak Borealis member ways, labeled via `[rel_ref]` |

The `style-streets.xml` tiers were chosen from real archive counts via
`osmflat-taginfo` (see the project memory). Text styles need fonts registered —
the harness registers Homebrew's bundled DejaVu, overridable with
`MAPNIK_FONT_DIR`.

### Fixtures for hard-to-map geometry

`test/fixtures/` holds small real-world extracts of edge cases that stress
ring assembly beyond simple multipolygon-with-holes. Each fixture is checked
in as four forms built from the same source data:

| file | what it is |
|------|------------|
| `<name>.osm.pbf`   | the raw extract |
| `<name>.geojson`   | `osmium export`, for the mapnik `geojson.input` plugin — an independently-implemented area assembler to cross-check against |
| `<name>.osm.flat/` | `osmflatc` archive, for the osmflat plugin |
| `<name>.osm.ext/`  | `osmflat-extc --taginfo` sidecar (for the `ext`/`tags` push-down params) |

| fixture | why it's hard |
|---------|---------------|
| `baarle-hertog` | Baarle-Hertog (Belgium) / Baarle-Nassau (Netherlands) enclave complex: a single `boundary` relation whose "outer" members form ~20+ **disjoint** closed rings (scattered exclaves), not one ring with holes. `osmium export`'s independent area assembler agrees exactly: 25 polygons for Baarle-Hertog, matching `diag_sd`'s ring count. One of those 25 also has 6 holes (the "counter-enclave" pattern — Dutch parcels inside a Belgian exclave), so this fixture covers hole-nesting too |
| `bicycle-route`  | a small Dutch node-network cycle route relation (`type=route`, `route=bicycle`, 11 way members) plus ~260 nearby highway ways that are *not* members — the negative control needed to test that `member_of` actually excludes, not just includes |

Render a fixture's osmflat archive:

```sh
./build/render ./build/plugins ./test/style-relations.xml \
    test/fixtures/baarle-hertog.osm.flat baarle.png 4.75 51.38 5.02 51.49
```

### osmflat vs. geojson.input equivalence (`compare_plugins`)

`compare_plugins` (built alongside `render` under `BUILD_RENDER_TEST`) drives
the osmflat plugin and mapnik's `geojson.input` plugin directly through
`datasource::features()` — no rendering, no style XML — and diffs the
polygon/multi_polygon features each one assembles from a `.osm.flat` archive
and its `.geojson` sibling:

```sh
./build/compare_plugins ./build/plugins /opt/homebrew/lib/mapnik/input \
    test/fixtures/baarle-hertog.osm.flat test/fixtures/baarle-hertog.geojson \
    4.75 51.38 5.02 51.49
```

It matches features by `name`+`admin_level` and compares, per matched pair:
polygon count, ring count per polygon, and per-ring shoelace area (within a
tolerance, default 1%, overridable as a trailing arg). It deliberately does
**not** diff rendered pixels or raw point counts — see the "byte-for-byte"
discussion in the project memory for why those are too brittle (draw-order
and ring-closing-stitch differences between two independent assemblers
change pixels/vertices without changing the shape). On `baarle-hertog` all
four relations match with the two `osmflat`/`osmflat-ext` sidecar's stitched
rings landing within 0.23% of osmium's independently-assembled area.

Two gotchas this harness needed to work around, in case you extend it:
- **osmflat's attributes are query-driven** (see mapnik-gotchas memory): a
  bare `mapnik::query(bbox)` won't populate `name`/`admin_level` on osmflat
  features — call `query::add_property_name()` for every tag you read.
  `geojson.input` always exposes every property regardless, so this is a
  no-op on that side.
- The GeoJSON side returns every node/way/relation as Point/LineString/
  (Multi)Polygon features in one file; only the last are comparable to
  osmflat's `osm_type=relation` query, so the harness filters by geometry
  type (empty polygon list = skip) rather than by tag presence.

### `member_of` correctness (`check_member_of`)

`member_of` has no equivalent in `geojson.input` (or any other stock mapnik
plugin) to diff against, so `check_member_of` checks the osmflat plugin's own
contract directly against a hardcoded ground-truth member list instead of
comparing two implementations:

```sh
./build/check_member_of ./build/plugins test/fixtures/bicycle-route.osm.flat \
    4.64 51.35 4.74 51.43
```

Against `bicycle-route`, it checks: an unfiltered `osm_type=way` query returns
all 274 ways in the archive; `member_of=route=bicycle,ref=09-33` returns
*exactly* the 11 known member way ids (from the Overpass fetch, not anything
the plugin computed) each carrying `rel_ref=09-33` via the forward join; and
a `member_of` filter matching no relation (`ref=99-99`) returns zero ways
rather than silently passing everything through. That last case is the
actual bug this filter could plausibly have, and only has teeth because the
fixture's ~260 non-member highway ways give it something to wrongly include.

`bicycle-route.osm.flat` is built with `osmflatc --ids` (unlike
`baarle-hertog`) so features carry their original OSM way id for the
ground-truth comparison.

`test/fixtures/fetch-<name>.sh` scripts document how each fixture was pulled
from Overpass and regenerate all four forms if OSM data changes (verified
byte-identical on rerun). They `osmium sort` then `osmium extract
--set-bounds` before handing off to `osmflatc` — without `--set-bounds` the
PBF header carries no bounding box, and since `osmflat_archive_envelope()`
reads the bbox straight from the header (never computed from node
coordinates), the plugin's envelope collapses to `(0,0,0,0)` and every render
comes back **silently blank**, regardless of query bbox or style.

### Tag push-down (fast wide zoom)

Without help, the datasource returns **all** primitives in the bbox and mapnik
applies `<Filter>`/`<MaxScaleDenominator>` at render time, so a state view walks
every way/node even though most are gated out.

The `ext` + `tags` params fix this: point `ext` at the archive's Ext sidecar
(`osmflat-ext`, built with `osmflat-extc --taginfo`) and set `tags` to a prefilter
that is a **superset** of the layer's rule filters (e.g. `highway=*` for a roads
layer). The plugin then intersects the sidecar's inverted-index postings with the
bbox spatial ranges (`osmflat_ext::query::intersect_bbox`, `O(R·log k)`), running
the bbox scan once and materializing only matching entities. On a ~5° state view
this cut render time from **≈32 s → ≈7.5 s** with byte-identical output.

`tags` must stay a superset of what the rules match, or you'll drop features; it's
a performance hint, not a substitute for `<Filter>`. See `test/style-full.xml`,
whose layers set `ext`/`tags` per layer.

### Relation membership (`member_of`)

Route relations (`type=route`) carry the interesting tags (`ref=Borealis`,
`network=Amtrak`) but no drawable geometry of their own — the rails are member
ways. `member_of` styles those members:

```xml
<Parameter name="osm_type">way</Parameter>
<Parameter name="tags">railway=rail</Parameter>
<Parameter name="member_of">route=train,ref=Borealis</Parameter>
```

emits only the ways that are members of a relation matching **all** the
`member_of` terms (AND — unlike `tags`, whose terms union), as ordinary line
geometry. The matched parent relation's tags are exposed to the style as
`rel_`-prefixed attributes (`[rel_ref]`, `[rel_name]`, …), while unprefixed
keys still read the member's own tags. One feature is emitted per
(member × matched relation) pair, so a way on two matching routes appears once
per route with that route's `rel_*` values. See `test/style-borealis.xml`.

Notes:
- Works for node members too (`osm_type=node` → stop positions).
- `member_of` composes with `tags` and the bbox (all must pass). It is
  enforced even without the `ext` sidecar via a full relation scan (slower —
  the sidecar's inverted index makes the relation match cheap); `tags` remains
  sidecar-only either way.
- `osm_type=relation` emission is unaffected: membership does not recurse, so
  relation-as-member (e.g. a `route_master`'s routes) is skipped, and
  area relations still render through the multipolygon path.
- Keep `member_of` selective (a specific route, network, or operator): all
  members of every matching relation are expanded before the bbox clips them.
- While `member_of` is active, a genuine OSM tag literally named `rel_*` on a
  member is shadowed by the parent-relation redirect.

The `simplify` param (default 0.5 px, on) generalizes geometry to sub-pixel using
the query resolution — scale-aware, so it self-adjusts at every zoom. It's
visually lossless and reduces the vertex count mapnik rasterizes, but note it runs
*after* materialization, so it doesn't cut the wide-zoom bottleneck (materializing
the pushed-down features) — tightening `tags` is the lever for that. `way_area` is
computed from full-resolution geometry, before simplification.

### Correlation dump (`dump`)

Mapnik's SVG output has no per-feature identity — it's flattened to bare
`<path>`/`<text>` elements, so a downstream tool can't tell which path is
"Hennepin Ave" versus an anonymous fragment. `dump=<path>` has the datasource
append one NDJSON line per emitted feature to that path, each line a GeoJSON
`Feature` carrying exactly what was handed to mapnik *before* projection,
clipping, or symbolizing touched it:

```json
{"type":"Feature","properties":{"osm_type":"way","osm_id":493358990,"z_order":70,"way_area":0,"is_closed":false,"highway":"primary","name":"Carretera México - Cuernavaca (Libre)","ref":"MEX 95","oneway":"no"},"geometry":{"type":"LineString","coordinates":[[-99.1649325,19.1995016],[-99.1615194,19.2015559]]}}
```

Geometry is raw `EPSG:4326` (lon/lat), matching the datasource's declared SRS —
an offline post-processor reprojects and affine-transforms it into the same
space as the rendered SVG to match dumped features back to output paths
(bbox-clipping can still split one feature into several path fragments; this
dump is what lets those be reassembled by identity instead of guessed at
geometrically).

Properties are wider than the style's own attributes: alongside whatever tags
the active style references, every feature also carries a fixed
naming/classifying allowlist (`name`, `ref`, `highway`, `waterway`, `railway`,
`admin_level`, `building`, …) regardless of whether any rule filters on them —
otherwise a style that only ever writes `[highway] = 'primary'` would never
pull `name`, and the dump would have nothing to call the road. This widened
fetch only feeds the dump; it does not change the mapnik attribute schema the
style sees.

The file is opened in append mode, so multiple `<Layer>`s (or multiple queries
against one layer) can all point `dump` at the same path without truncating
each other — clear the file yourself before a fresh render run. Point queries
(`features_at_point`, interactive lookups) never write to the dump; it's meant
to correlate one bulk render, not ad hoc queries.

### Debug logging

The plugin logs one line per query through mapnik's own `MAPNIK_LOG_DEBUG`
machinery — off by default (compiled to a genuine no-op unless `MAPNIK_LOG` is
defined, which this plugin's `CMakeLists.txt` does) and gated at runtime by
the usual `mapnik::logger` severity, same as every other mapnik plugin's debug
output. Two lines per query, from `osmflat_datasource::features()` /
`features_at_point()` and from `osmflat_featureset`'s destructor:

```
osmflat: query bbox=[-73.9,40.7575,-73.84,40.8025] osm_type=way tags=natural=bay member_of=- order=none simplify_px=0.5 style_keys=1
osmflat: featureset closed, emitted=3 features
```

The first line is everything the query asked for (bbox, `osm_type`, `tags`,
`member_of`, `order`, `simplify`, how many style keys were requested); the
second is how many features the returned cursor actually yielded, logged when
mapnik is done with it — whether it drained the cursor to exhaustion or
stopped early. Useful for confirming a `tags`/`member_of` filter is actually
selecting what you think it is, or for noticing that a layer you expected to
be empty (or non-empty) isn't, without adding throwaway `[osm_id]` text rules
to a style just to see what came back.

To turn it on, the *caller* still has to raise mapnik's log severity — this
plugin only emits at debug level, it doesn't change the global severity
itself. `test/render.cpp` (the smoke-test renderer, and what `scripts/render.sh`
in the styles repo shells out to) does this when `OSMFLAT_LOG_DEBUG` is set:

```
OSMFLAT_LOG_DEBUG=1 render.sh style.xml out.png <bbox...>
```

Other hosts (e.g. `mapnik-config`-based mod_tile setups, or your own harness)
can do the same with `mapnik::logger::set_severity(mapnik::logger::debug)`
before rendering.

Separately, `osmflat-capi`'s relation assembly (`assemble_multipolygon`) prints
a raw `eprintln!` — not routed through `MAPNIK_LOG` at all — when a relation's
outer ways don't form a closed ring and it has to drop the relation entirely:

```
osmflat-mapnik-plugin: dropping relation id=Some(15624542) name="": outer ways don't form a closed ring
```

This is real signal (a relation that should have area got silently dropped)
but it's easy to miss since it's unconditional Rust-side stderr output, not
gated by severity like the C++ side's logging. Worth promoting to a proper
`MAPNIK_LOG_WARN` through the C API at some point rather than a bare
`eprintln!`.

### Precomputed multipolygon assembly (`ext`'s `--multipolygons`)

Assembling a `type=multipolygon`/`boundary` relation's outer/inner ways into
closed rings is real stitching work — matching endpoints, picking the nearest
candidate when several are in range, deciding when a ring is actually
closed — and doing it live, on every single query that touches an area
relation, is more fragile than it looks: this exact algorithm had a real
premature-ring-closure bug that only showed up on one specific real,
jagged coastline (Elliott Bay, Seattle) after passing every synthetic test
thrown at it. The wider OSM rendering ecosystem doesn't do this assembly live
either — `osmcoastline` assembles coastline rings once, offline, and renderers
just read the already-valid result.

When the archive's `ext` sidecar was built with `osmflat-extc --multipolygons`,
this plugin does the same thing: `materialize_relation` reads the precomputed
rings directly (no re-stitching) instead of calling
`osmflat_ext::multipolygon::assemble_multipolygon` live. Confirmed
bit-for-bit identical rendering against the live path on real data (NYC
boroughs, Seattle water bodies including a polygon-with-a-hole case, and the
Elliott Bay edge case) before and after this was wired in.

One deliberate behavior difference: the precomputed sidecar doesn't carry a
relation's leftover *open* chains (the rare case where some, but not all, of
a relation's outer ways stitch into a closed ring) — only closed polygons.
Live assembly emits those leftovers as an extra unfilled `LineString` feature
(`closed=false`, `way_area=0`) so a diagnostic style can still see them; with
the precomputed sidecar active, a relation with no closed polygon simply
yields no features at all, and its "dropping relation" stderr line above
fires even for relations that live assembly wouldn't have dropped outright.
This never affects a style that filters by tag (nothing about those leftovers
was going to render anyway), but a diagnostic style that renders every
relation regardless of tags — like the water-body diagnostics used to find
the Elliott Bay and Bowery Bay issues in the first place — will see less
under `--multipolygons` than under live assembly. Build without
`--multipolygons` (or query `assemble_multipolygon` directly) if you need
those leftovers.

See `osmflat-ext`'s README for the build side (`osmflat-extc --multipolygons`)
and `osmflat_ext::multipolygon`'s module docs for the algorithm and its test
coverage (including a regression test for the premature-closure bug).

### Synthetic land polygons (`_osmflat_land=yes`)

`natural=coastline` ways mark a boundary, not an area — there's no
`natural=water`-style polygon a renderer can just fill for "everything on the
sea side," so a style that only understands ordinary tagged areas renders open
ocean, bays, and the water side of any coastline as blank background. This
plugin fills that gap with a synthetic land layer: any `<Layer>` whose
`Datasource` `tags` includes the magic pair `_osmflat_land=yes` (real OSM data
can never carry a leading-underscore key, so this can't collide) gets back a
`LineString` feature per land ring, closed, with `way_area` set — pair it with
`order=way_area` and a background-color map so a plain painter's algorithm
handles nesting (islands in bays, lakes on islands) correctly with no explicit
hole/exterior pairing.

Two sidecar sources can back this, checked in order:

1. **`ext`'s `--land-polygons`** (preferred when present): rings imported at
   build time from an external, already-closed coastline dataset — e.g.
   osmdata.openstreetmap.de's `land-polygons` product, the same one
   `osm2pgsql`/`openstreetmap-carto` production stacks use — reprojected from
   Web Mercator to WGS84. Unlike coastline ring assembly below, this dataset
   is already closed for **mainland** coastlines too (a real country's coast
   is an open chain thousands of kilometers long across a `natural=coastline`
   scan of any bounded extract; there's no tile/bbox frame to close it
   against that isn't arbitrary), so this is what actually shows mainland USA
   as land rather than leaving it as open water. Traded off against
   `--coastline`: coarser geometry (the "simplified" variant), since it isn't
   derived from the archive's own full-resolution `natural=coastline` ways —
   in practice this hasn't cost visible precision even at NYC's Harlem River /
   the Narrows or Seattle's Puget Sound shoreline (validated against both).
2. **`ext`'s `--coastline`** (fallback when `--land-polygons` wasn't built):
   rings assembled from the parent archive's own `natural=coastline` ways,
   classified land/water by winding and sorted by area descending. Only ever
   produces **closed** rings — islands, lakes fully enclosed by coastline —
   because the ring assembler has nothing to close an open mainland chain
   against; a sidecar built with `--coastline` alone will correctly show NYC's
   islands and inter-borough rivers as land/water but can never show a
   mainland coastline as land at all.

See `osmflat-ext`'s README for both build sides and `osmflat_ext::coastline` /
`osmflat_ext::land_polygons`'s module docs for the respective algorithms.
