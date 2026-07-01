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

The datasource is **semantically neutral**: nodes → points, ways → line strings
(open *and* closed alike — no area heuristics). The style decides fill vs. stroke
per tag (mapnik's `PolygonSymbolizer` fills a closed ring, `LineSymbolizer`
strokes it). `type=multipolygon`/`boundary` **relations** are assembled from their
member ways into `multi_polygon` geometry (outer rings + holes).

**Attributes are query-driven:** each feature exposes exactly the tags the active
style references (`[highway]`, `[natural]`, …), null-filled when absent, plus
synthetic geometric facts: `osm_id` (Integer, real OSM id), `osm_type`
(String: node/way/relation), `is_closed` (Boolean).

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

The `style-streets.xml` tiers were chosen from real archive counts via
`osmflat-taginfo` (see the project memory). Text styles need fonts registered —
the harness registers Homebrew's bundled DejaVu, overridable with
`MAPNIK_FONT_DIR`.

### Performance at wide zoom

The datasource answers a bbox query by returning **all** primitives in the box;
it can't push a tag filter (e.g. `highway=motorway`) down into the query — mapnik
applies `<Filter>`/`<MaxScaleDenominator>` at render time, *after* the features
are materialized. So a state/country-sized view still walks every way/node in the
box even though most are gated out, making wide zooms slow (≈18 s for a ~1.7°
region). It renders correctly; it just isn't fast. The real fix is a datasource
push-down (a highway-class / tag prefilter, or a pre-generalized overview archive)
— noted as future work.
