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
| `osm_type` | no       | `node`\|`way`\|`all`| primitives to emit (default all) |

Nodes are emitted as points, ways as line strings, each carrying its OSM tags
as feature attributes plus an `osm_id` field. (Relation/polygon support is not
yet implemented.)

## Smoke test

```sh
cmake -S . -B build -DBUILD_RENDER_TEST=ON
cmake --build build
./build/render <archive_dir> ./build/plugins out.png <minx> <miny> <maxx> <maxy>
# e.g. Mexico City:
./build/render ../osmflat-rs/mexico.osm.flat ./build/plugins mexico.png \
    -99.30 19.20 -98.95 19.60
```
