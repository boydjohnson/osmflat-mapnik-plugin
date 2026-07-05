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
                    osmflat crate  (../osmflat-rs, feature/spatial-index)
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

Requirements: a C++17 compiler, CMake ≥ 3.22, Rust/Cargo, mapnik (`libmapnik`
discoverable via `pkg-config`), and Boost headers. On macOS/Homebrew the Boost
include path is added automatically via `brew --prefix`.

## Datasource parameters

| param      | required | values              | meaning                          |
|------------|----------|---------------------|----------------------------------|
| `type`     | yes      | `osmflat`           | selects this plugin              |
| `file`     | yes      | path                | the `*.osm.flat` archive dir     |
| `osm_type` | no       | comma-separated `node`\|`way`\|`relation`\|`all` | primitives to emit (default all); e.g. `way,relation` |
| `ext`      | no       | path                | Ext sidecar dir (`*.osm.ext`) enabling tag push-down |
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

`test/fixtures/` holds small `.osm.pbf` extracts of real-world edge cases that
stress ring assembly beyond simple multipolygon-with-holes:

| fixture | why it's hard |
|---------|---------------|
| `baarle-hertog.osm.pbf` | Baarle-Hertog (Belgium) / Baarle-Nassau (Netherlands) enclave complex: a single `boundary` relation whose "outer" members form ~20+ **disjoint** closed rings (scattered exclaves), not one ring with holes |

Rebuild the archive from a fixture and render it, e.g.:

```sh
cargo run --release --manifest-path ../osmflat-rs/osmflatc/Cargo.toml -- \
    test/fixtures/baarle-hertog.osm.pbf /tmp/baarle.osm.flat
./build/render ./build/plugins ./test/style-relations.xml \
    /tmp/baarle.osm.flat baarle.png 4.75 51.38 5.02 51.49
```

`test/fixtures/fetch-<name>.sh` scripts document how each fixture was pulled
from Overpass and re-derive it if OSM data changes. They `osmium sort` then
`osmium extract --set-bounds` before handing off to `osmflatc` — without
`--set-bounds` the PBF header carries no bounding box, and since
`osmflat_archive_envelope()` reads the bbox straight from the header (never
computed from node coordinates), the plugin's envelope collapses to
`(0,0,0,0)` and every render comes back **silently blank**, regardless of
query bbox or style.

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
