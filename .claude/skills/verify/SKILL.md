---
name: verify
description: Verify osmflat-mapnik-plugin changes end-to-end by rendering real archives through the mapnik plugin with build/render.
---

# Verifying osmflat-mapnik-plugin changes

The surface is a mapnik render through the real plugin (`osmflat.input`
loaded via `datasource_cache`), driven by the `build/render` harness.

## Build

```sh
cmake --build build          # builds Rust staticlib (Corrosion) + plugin + render
```

One step; cbindgen regenerates `rust/osmflat-capi/include/osmflat_capi.hpp`
first, so FFI signature drift fails loudly at C++ compile time.

## Drive

```sh
./build/render build/plugins <style.xml> <archive-dir> out.png \
    <minx> <miny> <maxx> <maxy> [width height]
```

- Archives: `../osmflat-rs/{minnesota,south-dakota,us-midwest,mexico,belize,illinois}.osm.flat`,
  each with a `.osm.ext` sidecar sibling. The harness derives `@EXT@` by
  swapping `.flat`→`.ext`; override with `OSMFLAT_EXT=...` (point it at a
  nonexistent path to exercise the no-sidecar fallback).
- Styles use `@ARCHIVE@`/`@EXT@` placeholders — `test/style-*.xml`, or sed a
  variant into the scratchpad for probes.
- Useful bboxes: Twin Cities `-94.2 44.6 -92.9 45.1` (minnesota); the Amtrak
  Borealis route enters at St. Paul Union Depot and runs southeast.
- Bbox coords are degrees (longlat map); scale_denominator is degree-based, so
  MaxScaleDenominator thresholds in styles are tiny numbers.

## Inspect

- Read the PNG directly (multimodal) for layout-level checks.
- For "is it blank" / pixel counts, decode with the stdlib-only PNG
  unfilter+count-non-white python snippet (no PIL installed).
- `cargo run --release --example member_query -- <archive> key=val,...`
  dumps relations matching tag terms (tags + member counts) — handy for
  picking real fixture entities before writing a style.

## Regression styles

`test/style-full.xml` (St. Paul, tags/ext push-down layers) and
`test/style-relations.xml` (multipolygon fills) are good no-crash/looks-sane
sweeps after datasource changes.
