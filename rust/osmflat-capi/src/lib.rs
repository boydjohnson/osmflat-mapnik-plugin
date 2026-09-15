//! C ABI over the `osmflat` spatial-query API, consumed by the C++ mapnik
//! datasource plugin in `../../src`.
//!
//! Design (see the project's attribute-geometry-design notes): the datasource
//! is *semantically neutral*. Geometry is a pure geometric fact —
//! node → point, way → line string (open or closed alike) — and all OSM meaning
//! is left to the mapnik style via tags. Attributes are *query-driven*: the C++
//! side passes down exactly the tag keys the active style references
//! (`query::property_names()`), and each feature carries a value (or "absent")
//! for each, plus a few synthetic geometric facts (`osm_id`, `osm_type`,
//! `is_closed`).
//!
//! Each `osmflat_query` materializes matching features into an owned
//! `OsmflatFeatureSet`; the C++ `Featureset::next()` pulls them one at a time.
//! Every pointer handed to C++ is owned by the feature set and stays valid until
//! the next `osmflat_featureset_next` or `osmflat_featureset_free`.

use std::ffi::CStr;
use std::os::raw::c_char;
use std::slice;

use osmflat::{
    find_nodes_by_bounding_box, find_relations_by_bounding_box, find_tag,
    find_ways_by_bounding_box, node_id, relation_id, way_id, FileResourceStorage, Node, Osm,
    Relation, RelationMembersRef, Way,
};
use osmflat_ext::query::Bbox;
use osmflat_ext::taginfo::TaginfoQuery;
use osmflat_ext::{Ext, ExtArchive};

/// Geometry kind of a materialized feature.
#[repr(u32)]
pub enum OsmflatGeomType {
    Point = 1,
    LineString = 2,
    Polygon = 3,
    /// Assembled from a `type=multipolygon`/`boundary` relation's member ways.
    MultiPolygon = 4,
}

/// OSM primitive a feature came from.
#[repr(u32)]
pub enum OsmflatOsmType {
    Node = 0,
    Way = 1,
    Relation = 2,
}

/// Draw order applied to the returned features (the `order` datasource param).
#[repr(u32)]
pub enum OsmflatOrder {
    /// Spatial (space-filling-curve) order — the default, no sorting.
    None = 0,
    /// Ascending `z_order`: minor features under major, bridges last (roads).
    ZOrder = 1,
    /// Descending `way_area`: large areas under small, so small stay visible.
    WayArea = 2,
}

/// A borrowed key handed *in* from C++ (a name from `query::property_names()`),
/// as raw UTF-8 bytes without a trailing NUL.
#[repr(C)]
pub struct OsmflatStrRef {
    pub ptr: *const u8,
    pub len: usize,
}

/// A borrowed tag prefilter term handed *in* from C++: `key=value`, or `key=*`
/// (any value) when `val` is empty. Used to push tag-filtering into the query
/// via the Ext inverted index.
#[repr(C)]
pub struct OsmflatKvRef {
    pub key: OsmflatStrRef,
    pub val: OsmflatStrRef,
}

unsafe fn str_ref<'a>(s: &OsmflatStrRef) -> Option<&'a [u8]> {
    if s.ptr.is_null() {
        None
    } else {
        Some(std::slice::from_raw_parts(s.ptr, s.len))
    }
}

/// Parse a borrowed `OsmflatKvRef` array: each term is `(key, Some(value))`
/// for `key=value`, or `(key, None)` for `key=*` (zero-length value).
unsafe fn kv_refs<'a>(ptr: *const OsmflatKvRef, num: usize) -> Vec<(&'a [u8], Option<&'a [u8]>)> {
    if num == 0 || ptr.is_null() {
        return Vec::new();
    }
    slice::from_raw_parts(ptr, num)
        .iter()
        .filter_map(|f| {
            let key = str_ref(&f.key)?;
            let val = str_ref(&f.val);
            Some((key, val.filter(|v| !v.is_empty())))
        })
        .collect()
}

/// True if `_osmflat_land` (any value, or `key=*`) is among the query's
/// `tags` filter terms -- the opt-in signal for synthetic coastline land
/// polygons. A real OSM entity can never carry this key (leading underscore),
/// so it can share the same `tags` list as ordinary prefilter terms without
/// any risk of colliding with real data.
fn wants_land(filters: &[(&[u8], Option<&[u8]>)]) -> bool {
    filters
        .iter()
        .any(|&(k, v)| k == b"_osmflat_land" && v.is_none_or(|v| v == b"yes"))
}

/// True if a ring's own bounding box (computed from its vertices; coastline
/// rings have no spatial index of their own) overlaps the query bbox.
fn ring_intersects_bbox(vertices: &[(f64, f64)], bbox: Bbox) -> bool {
    let (mut min_lon, mut min_lat) = (f64::INFINITY, f64::INFINITY);
    let (mut max_lon, mut max_lat) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for &(lon, lat) in vertices {
        min_lon = min_lon.min(lon);
        min_lat = min_lat.min(lat);
        max_lon = max_lon.max(lon);
        max_lat = max_lat.max(lat);
    }
    min_lon <= bbox.max_lon && max_lon >= bbox.min_lon && min_lat <= bbox.max_lat && max_lat >= bbox.min_lat
}

/// A borrowed attribute value handed *out* to C++. `present == false` means the
/// feature has no such tag (render as `value_null`); otherwise `ptr`/`len` are
/// the UTF-8 value bytes, valid until the next `osmflat_featureset_next` / free.
#[repr(C)]
pub struct OsmflatValue {
    pub present: bool,
    pub ptr: *const u8,
    pub len: usize,
}

/// Opaque archive handle. Owns the memory-mapped `Osm` archive, and optionally
/// its `Ext` sidecar (for tag-filter push-down via the inverted index).
pub struct OsmflatArchive {
    kind: ArchiveKind,
    coord_scale: f64,
}

enum ArchiveKind {
    Plain(Osm),
    Ext(ExtArchive),
}

impl OsmflatArchive {
    fn osm(&self) -> &Osm {
        match &self.kind {
            ArchiveKind::Plain(o) => o,
            ArchiveKind::Ext(e) => e.parent(),
        }
    }

    /// Inverted-tag-index query, when an Ext sidecar with `--taginfo` is loaded.
    fn taginfo(&self) -> Option<TaginfoQuery<'_>> {
        match &self.kind {
            ArchiveKind::Ext(e) => e.taginfo(),
            ArchiveKind::Plain(_) => None,
        }
    }

    /// Precomputed multipolygon relation query, when an Ext sidecar with
    /// `--multipolygons` is loaded. `None` falls back to live per-query
    /// assembly in `materialize_relation`.
    fn multipolygons(&self) -> Option<osmflat_ext::multipolygon::MultipolygonsQuery<'_>> {
        match &self.kind {
            ArchiveKind::Ext(e) => e.multipolygons(),
            ArchiveKind::Plain(_) => None,
        }
    }

    /// Precomputed coastline ring query, when an Ext sidecar with
    /// `--coastline` is loaded. `None` means no synthetic land features are
    /// ever added, regardless of what a style asks for.
    fn coastline(&self) -> Option<osmflat_ext::coastline::CoastlineQuery<'_>> {
        match &self.kind {
            ArchiveKind::Ext(e) => e.coastline(),
            ArchiveKind::Plain(_) => None,
        }
    }

    /// Imported external land-polygon query, when an Ext sidecar with
    /// `--land-polygons` is loaded. Preferred over `coastline()` for
    /// synthetic land features -- unlike the coastline ring assembler, which
    /// only ever emits closed rings (islands and lakes), this dataset is
    /// already closed for mainland coastlines too.
    fn land_polygons(&self) -> Option<osmflat_ext::land_polygons::LandPolygonsQuery<'_>> {
        match &self.kind {
            ArchiveKind::Ext(e) => e.land_polygons(),
            ArchiveKind::Plain(_) => None,
        }
    }
}

struct OwnedFeature {
    /// Real OSM id, or `None` if the archive has no `ids` sub-archive.
    osm_id: Option<u64>,
    osm_type: OsmflatOsmType,
    geom_type: OsmflatGeomType,
    is_closed: bool,
    /// osm2pgsql-style render priority: `layer*10000 + bridge/tunnel band +
    /// highway class rank`. Higher draws on top; used by the `order` param.
    z_order: i32,
    /// Enclosed area in square meters (spherical), for closed ways and
    /// multipolygon relations; 0 for points and open ways.
    way_area: f64,
    /// Point/line vertices, interleaved `[x0, y0, x1, y1, ...]` in degrees
    /// (EPSG:4326). Empty for `MultiPolygon` features (see `polygons`).
    coords: Vec<f64>,
    /// `MultiPolygon` geometry: `polygons[p][r]` is ring `r` of polygon `p` as
    /// interleaved coords; ring 0 is the exterior, rings 1.. are holes. Empty
    /// for point/line features.
    polygons: Vec<Vec<Vec<f64>>>,
    /// Values aligned to the query's requested keys; `None` == tag absent.
    attrs: Vec<Option<Vec<u8>>>,
}

/// Opaque, owned result of one bounding-box query.
pub struct OsmflatFeatureSet {
    features: Vec<OwnedFeature>,
    /// Index of the *next* feature to yield; current is `features[pos - 1]`.
    pos: usize,
}

impl OsmflatFeatureSet {
    fn current(&self) -> Option<&OwnedFeature> {
        self.pos.checked_sub(1).and_then(|i| self.features.get(i))
    }
}

unsafe fn cstr<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        None
    } else {
        CStr::from_ptr(p).to_str().ok()
    }
}

/// Opens an osmflat archive directory, plus an optional Ext sidecar directory
/// (`ext_path`, may be null) that enables tag-filter push-down. If the sidecar
/// fails to open or its fingerprint doesn't match the parent, it is ignored and
/// the archive opens plain. Returns null only if the parent can't be opened.
/// Free with `osmflat_archive_free`.
///
/// # Safety
/// `path` must be a valid, NUL-terminated C string; `ext_path` null or likewise.
#[no_mangle]
pub unsafe extern "C" fn osmflat_archive_open(
    path: *const c_char,
    ext_path: *const c_char,
) -> *mut OsmflatArchive {
    let Some(path) = cstr(path) else {
        return std::ptr::null_mut();
    };
    let osm = match Osm::open(FileResourceStorage::new(path)) {
        Ok(a) => a,
        Err(_) => return std::ptr::null_mut(),
    };
    let coord_scale = osm.header().coord_scale() as f64;

    let kind = match cstr(ext_path) {
        Some(ext_path) => match Ext::open(FileResourceStorage::new(ext_path)) {
            // ExtArchive::open consumes `osm` and verifies the fingerprint.
            Ok(ext) => match ExtArchive::open(osm, ext) {
                Ok(ext_archive) => ArchiveKind::Ext(ext_archive),
                Err(_) => match Osm::open(FileResourceStorage::new(path)) {
                    Ok(reopened) => ArchiveKind::Plain(reopened),
                    Err(_) => return std::ptr::null_mut(),
                },
            },
            Err(_) => ArchiveKind::Plain(osm),
        },
        None => ArchiveKind::Plain(osm),
    };

    Box::into_raw(Box::new(OsmflatArchive { kind, coord_scale }))
}

/// Frees an archive handle.
///
/// # Safety
/// `archive` must be from `osmflat_archive_open` (or null) and unused afterward.
#[no_mangle]
pub unsafe extern "C" fn osmflat_archive_free(archive: *mut OsmflatArchive) {
    if !archive.is_null() {
        drop(Box::from_raw(archive));
    }
}

/// Writes the archive bounding box (degrees) into the out pointers. Returns
/// false if `archive` is null.
///
/// # Safety
/// All out pointers must be valid for writes.
#[no_mangle]
pub unsafe extern "C" fn osmflat_archive_envelope(
    archive: *const OsmflatArchive,
    min_x: *mut f64,
    min_y: *mut f64,
    max_x: *mut f64,
    max_y: *mut f64,
) -> bool {
    let Some(archive) = archive.as_ref() else {
        return false;
    };
    let header = archive.osm().header();
    let scale = archive.coord_scale;
    min_x.write(header.bbox_left() as f64 / scale);
    min_y.write(header.bbox_bottom() as f64 / scale);
    max_x.write(header.bbox_right() as f64 / scale);
    max_y.write(header.bbox_top() as f64 / scale);
    true
}

/// Look up the requested tag values for `range`, aligned to `keys`. When
/// `rel_tags` is set (a `member_of` query), keys prefixed `rel_` are answered
/// from the matched parent relation's tag range instead of the feature's own —
/// so a style can reference `[rel_ref]`/`[rel_name]` on member geometry. While
/// active, this shadows any genuine OSM tag literally named `rel_*`.
fn collect_attrs(
    archive: &Osm,
    range: std::ops::Range<u64>,
    rel_tags: Option<&std::ops::Range<u64>>,
    keys: &[&[u8]],
) -> Vec<Option<Vec<u8>>> {
    keys.iter()
        .map(|key| {
            let (range, key) = match (rel_tags, key.strip_prefix(b"rel_".as_slice())) {
                (Some(r), Some(stripped)) => (r, stripped),
                _ => (&range, *key),
            };
            find_tag(archive, range.clone(), key).map(|v| v.to_vec())
        })
        .collect()
}

/// Runs a bounding-box query and returns an owned feature set. `min_*`/`max_*`
/// are degrees (lon = x, lat = y). `include_nodes`/`include_ways`/
/// `include_relations` select which primitives to emit (a performance filter,
/// not semantics); relations are emitted only for `type=multipolygon`/`boundary`
/// as assembled multipolygons. `keys`/`num_keys` are the tag names to
/// materialize (from `query::property_names()`, synthetic names already stripped
/// by the caller). A relation can emit multiple features when an incomplete
/// multipolygon has both closed rings and unclosed outer chains. Free with
/// `osmflat_featureset_free`.
///
/// `member_filters` (the `member_of` datasource param) is a relation-membership
/// filter: when non-empty, nodes/ways are emitted only if they are members of a
/// relation matching *all* the terms (AND — unlike `filters`, which unions),
/// one feature per (member × matched relation) pair, and `rel_`-prefixed keys
/// resolve against the matched parent relation's tags. It is enforced even
/// without an Ext sidecar (via a full relation scan). Relation emission is
/// unaffected — membership does not recurse.
///
/// # Safety
/// `archive` must be valid; `keys` must point to `num_keys` valid `OsmflatStrRef`
/// whose byte ranges are valid for the duration of the call; likewise `filters`
/// and `member_filters` with their counts.
#[no_mangle]
pub unsafe extern "C" fn osmflat_query(
    archive: *const OsmflatArchive,
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
    include_nodes: bool,
    include_ways: bool,
    include_relations: bool,
    keys: *const OsmflatStrRef,
    num_keys: usize,
    filters: *const OsmflatKvRef,
    num_filters: usize,
    member_filters: *const OsmflatKvRef,
    num_member_filters: usize,
    order: OsmflatOrder,
    simplify_tolerance: f64,
) -> *mut OsmflatFeatureSet {
    let Some(handle) = archive.as_ref() else {
        return std::ptr::null_mut();
    };
    let archive = handle.osm();
    let scale = handle.coord_scale;

    let key_refs: Vec<&[u8]> = if num_keys == 0 {
        Vec::new()
    } else {
        slice::from_raw_parts(keys, num_keys)
            .iter()
            .map(|k| {
                if k.ptr.is_null() {
                    &[][..]
                } else {
                    slice::from_raw_parts(k.ptr, k.len)
                }
            })
            .collect()
    };

    let filter_refs = kv_refs(filters, num_filters);
    let member_filter_refs = kv_refs(member_filters, num_member_filters);

    let bbox = Bbox {
        min_lon: min_x,
        min_lat: min_y,
        max_lon: max_x,
        max_lat: max_y,
    };

    let mut features = Vec::new();

    // For each primitive: if a prefilter is set and the sidecar is present, walk
    // only the candidate indices from the inverted index ∩ bbox; otherwise scan
    // the full spatial query.
    if !member_filter_refs.is_empty() {
        // member_of: forward join. Match relations by tags (AND), expand their
        // node/way members, and keep only members that also pass the tag
        // prefilter (when sidecar-backed) and the bbox. One feature per
        // (member × matched relation) pair so `rel_*` attrs are well-defined.
        let rels = relations_matching_all(handle, &member_filter_refs);
        let (node_pairs, way_pairs) =
            expand_member_pairs(archive, &rels, include_nodes, include_ways);
        if include_nodes {
            let allowed = candidate_indices(handle, &filter_refs, bbox, Prim::Node)
                .unwrap_or_else(|| osmflat_ext::query::node_indices_in_bbox(archive, bbox));
            for (n, r) in node_pairs {
                if allowed.binary_search(&n).is_ok() {
                    let rel_tags = archive.relations()[r as usize].tags();
                    features.push(materialize_node(
                        archive,
                        n as usize,
                        scale,
                        &key_refs,
                        Some(&rel_tags),
                    ));
                }
            }
        }
        if include_ways {
            let allowed = candidate_indices(handle, &filter_refs, bbox, Prim::Way)
                .unwrap_or_else(|| osmflat_ext::query::way_indices_in_bbox(archive, bbox));
            for (w, r) in way_pairs {
                if allowed.binary_search(&w).is_ok() {
                    let rel_tags = archive.relations()[r as usize].tags();
                    if let Some(f) = materialize_way(
                        archive,
                        w as usize,
                        scale,
                        simplify_tolerance,
                        &key_refs,
                        Some(&rel_tags),
                    ) {
                        features.push(f);
                    }
                }
            }
        }
    } else {
        if include_nodes {
            match candidate_indices(handle, &filter_refs, bbox, Prim::Node) {
                Some(indices) => features.extend(
                    indices
                        .into_iter()
                        .map(|i| materialize_node(archive, i as usize, scale, &key_refs, None)),
                ),
                None => {
                    let base = archive.nodes().as_ptr();
                    for node in find_nodes_by_bounding_box(archive, min_x, min_y, max_x, max_y) {
                        let idx = (node as *const Node).offset_from(base) as usize;
                        features.push(materialize_node(archive, idx, scale, &key_refs, None));
                    }
                }
            }
        }

        if include_ways {
            match candidate_indices(handle, &filter_refs, bbox, Prim::Way) {
                Some(indices) => features.extend(indices.into_iter().filter_map(|i| {
                    materialize_way(
                        archive,
                        i as usize,
                        scale,
                        simplify_tolerance,
                        &key_refs,
                        None,
                    )
                })),
                None => {
                    let base = archive.ways().as_ptr();
                    for way in find_ways_by_bounding_box(archive, min_x, min_y, max_x, max_y) {
                        let idx = (way as *const Way).offset_from(base) as usize;
                        if let Some(f) = materialize_way(
                            archive,
                            idx,
                            scale,
                            simplify_tolerance,
                            &key_refs,
                            None,
                        ) {
                            features.push(f);
                        }
                    }
                }
            }
        }
    }

    if include_relations {
        let mp = handle.multipolygons();
        match candidate_indices(handle, &filter_refs, bbox, Prim::Relation) {
            Some(indices) => features.extend(indices.into_iter().flat_map(|i| {
                materialize_relation(archive, i as usize, scale, simplify_tolerance, &key_refs, mp)
            })),
            None => {
                let base = archive.relations().as_ptr();
                for relation in find_relations_by_bounding_box(archive, min_x, min_y, max_x, max_y)
                {
                    let idx = (relation as *const Relation).offset_from(base) as usize;
                    features.extend(materialize_relation(
                        archive,
                        idx,
                        scale,
                        simplify_tolerance,
                        &key_refs,
                        mp,
                    ));
                }
            }
        }
    }

    // Synthetic coastline "land" polygons: opt-in via a magic tag pair in the
    // `tags` filter (`_osmflat_land=yes`) rather than a new osm_type/C-ABI
    // parameter -- piggybacks entirely on the existing tags-filter plumbing,
    // so no signature changes anywhere in the C++/FFI boundary were needed.
    // Independent of `include_nodes`/`include_ways`/`include_relations`: a
    // style asking for land polygons gets them regardless of what else it
    // asked for. See osmflat-mapnik-plugin's README for the full picture
    // (mirrors `osmcoastline`'s land/water split, precomputed once offline).
    //
    // `land_polygons` (imported from an external, already-closed dataset) is
    // preferred over `coastline` (rings assembled from the parent's own
    // `natural=coastline` ways): the assembler only ever closes islands and
    // lakes -- open mainland chains are dropped, not emitted as land -- so a
    // sidecar built with `--coastline` alone can never show mainland as land.
    if wants_land(&filter_refs) {
        let land_key_idx = key_refs.iter().position(|k| *k == b"_osmflat_land");
        let mut push_land_ring = |vertices: &[(f64, f64)], area_m2: f64| {
            let mut attrs = vec![None; key_refs.len()];
            if let Some(i) = land_key_idx {
                attrs[i] = Some(b"yes".to_vec());
            }
            features.push(OwnedFeature {
                osm_id: None,
                osm_type: OsmflatOsmType::Way,
                geom_type: OsmflatGeomType::LineString,
                is_closed: true,
                z_order: 0,
                way_area: area_m2,
                coords: vertices.iter().flat_map(|&(x, y)| [x, y]).collect(),
                polygons: Vec::new(),
                attrs,
            });
        };

        if let Some(land_polygons) = handle.land_polygons() {
            for ring in land_polygons.rings() {
                if !ring.is_land || !ring_intersects_bbox(&ring.vertices, bbox) {
                    continue;
                }
                let area_m2 = osmflat_ext::coastline::signed_area_m2(&ring.vertices).abs();
                push_land_ring(&ring.vertices, area_m2);
            }
        } else if let Some(coastline) = handle.coastline() {
            for ring in coastline.rings() {
                if !ring.is_land || !ring_intersects_bbox(&ring.vertices, bbox) {
                    continue;
                }
                push_land_ring(&ring.vertices, ring.area_m2);
            }
        }
    }

    // Apply the requested draw order (mapnik renders in the returned order).
    match order {
        OsmflatOrder::ZOrder => features.sort_by_key(|f| f.z_order),
        OsmflatOrder::WayArea => features.sort_by(|a, b| {
            b.way_area
                .partial_cmp(&a.way_area)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        OsmflatOrder::None => {}
    }

    Box::into_raw(Box::new(OsmflatFeatureSet { features, pos: 0 }))
}

/// Which primitive a candidate-index lookup targets.
#[derive(Clone, Copy)]
enum Prim {
    Node,
    Way,
    Relation,
}

/// Candidate parent indices for `prim` matching *any* of the `filters` within
/// `bbox`, using the Ext inverted index (`postings ∩ bbox` merge-join). Returns
/// `None` — meaning "fall back to a full spatial scan" — when there is no
/// prefilter or no sidecar loaded.
fn candidate_indices(
    handle: &OsmflatArchive,
    filters: &[(&[u8], Option<&[u8]>)],
    bbox: Bbox,
    prim: Prim,
) -> Option<Vec<u64>> {
    use osmflat_ext::query::{
        intersect_bbox, node_indices_in_bbox, relation_indices_in_bbox, to_index_ranges,
        way_indices_in_bbox,
    };

    if filters.is_empty() {
        return None;
    }
    let taginfo = handle.taginfo()?;
    let archive = handle.osm();

    // Run the bbox spatial scan ONCE and reuse its index ranges for every
    // postings intersection — crucial for `key=*`, which fans out over all of a
    // key's values (otherwise each value would re-scan the whole bbox).
    let idx = match prim {
        Prim::Node => node_indices_in_bbox(archive, bbox),
        Prim::Way => way_indices_in_bbox(archive, bbox),
        Prim::Relation => relation_indices_in_bbox(archive, bbox),
    };
    let ranges = to_index_ranges(&idx);

    let mut all: Vec<u64> = Vec::new();
    let mut add = |vv: &osmflat_ext::taginfo::ValueView| {
        let postings = match prim {
            Prim::Node => vv.nodes(),
            Prim::Way => vv.ways(),
            Prim::Relation => vv.relations(),
        };
        all.extend(intersect_bbox(postings, &ranges));
    };

    for (key, val) in filters {
        match val {
            Some(value) => {
                if let Some(vv) = taginfo.kv(key, value) {
                    add(&vv);
                }
            }
            // key=* : union the bbox intersection over all of the key's values.
            None => {
                if let Some(kview) = taginfo.key(key) {
                    for vv in kview.values() {
                        add(&vv);
                    }
                }
            }
        }
    }
    all.sort_unstable();
    all.dedup();
    Some(all)
}

/// Intersection of two ascending, deduped index lists (linear merge).
fn intersect_sorted_u64(a: &[u64], b: &[u64]) -> Vec<u64> {
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                out.push(a[i]);
                i += 1;
                j += 1;
            }
        }
    }
    out
}

/// Ascending relation indices whose tags match *all* of `filters` (AND — unlike
/// the `tags` prefilter, whose terms union). Uses the Ext inverted index when
/// loaded; otherwise falls back to a full relation scan with `find_tag`, so the
/// filter is enforced either way — a style cannot re-express relation
/// membership itself, unlike a tag filter.
fn relations_matching_all(handle: &OsmflatArchive, filters: &[(&[u8], Option<&[u8]>)]) -> Vec<u64> {
    if filters.is_empty() {
        return Vec::new();
    }
    let archive = handle.osm();
    let Some(taginfo) = handle.taginfo() else {
        // No sidecar: scan every relation (flatdata trims the range sentinel
        // from the slice, so every element is a real relation).
        return (0..archive.relations().len())
            .filter(|&i| {
                let tags = archive.relations()[i].tags();
                filters.iter().all(|(key, val)| match val {
                    Some(v) => find_tag(archive, tags.clone(), key) == Some(*v),
                    None => find_tag(archive, tags.clone(), key).is_some(),
                })
            })
            .map(|i| i as u64)
            .collect();
    };

    let mut term_sets: Vec<Vec<u64>> = Vec::with_capacity(filters.len());
    for (key, val) in filters {
        let set: Vec<u64> = match val {
            Some(value) => taginfo
                .kv(key, value)
                .map(|vv| vv.relations().iter().map(|r| r.value()).collect())
                .unwrap_or_default(),
            // key=* : union the relation postings over all of the key's values.
            None => taginfo
                .key(key)
                .map(|kview| {
                    let lists: Vec<&[osmflat_ext::Ref]> =
                        kview.values().map(|vv| vv.relations()).collect();
                    osmflat_ext::query::union(&lists).collect()
                })
                .unwrap_or_default(),
        };
        if set.is_empty() {
            return Vec::new();
        }
        term_sets.push(set);
    }
    // Intersect smallest-first to keep the accumulator minimal.
    term_sets.sort_by_key(|s| s.len());
    let mut iter = term_sets.into_iter();
    let mut acc = iter.next().unwrap();
    for set in iter {
        acc = intersect_sorted_u64(&acc, &set);
        if acc.is_empty() {
            break;
        }
    }
    acc
}

/// Distinct `(member_idx, relation_idx)` pairs for the node and way members of
/// `rels`, each list sorted so members stay in ascending (spatial) index order.
/// Relation-as-member is skipped: membership does not recurse, so e.g. a
/// `route_master` match yields nothing.
fn expand_member_pairs(
    archive: &Osm,
    rels: &[u64],
    want_nodes: bool,
    want_ways: bool,
) -> (Vec<(u64, u64)>, Vec<(u64, u64)>) {
    let members = archive.relation_members();
    let mut node_pairs = Vec::new();
    let mut way_pairs = Vec::new();
    for &r in rels {
        for member in members.at(r as usize) {
            match member {
                RelationMembersRef::NodeMember(m) if want_nodes => {
                    if let Some(n) = m.node_idx() {
                        node_pairs.push((n, r));
                    }
                }
                RelationMembersRef::WayMember(m) if want_ways => {
                    if let Some(w) = m.way_idx() {
                        way_pairs.push((w, r));
                    }
                }
                _ => {}
            }
        }
    }
    for pairs in [&mut node_pairs, &mut way_pairs] {
        pairs.sort_unstable();
        pairs.dedup();
    }
    (node_pairs, way_pairs)
}

/// Spherical area (m²) of a ring given as interleaved `[lon, lat, ...]` degrees.
/// Sign-independent (returns the absolute area), so winding order doesn't matter.
fn ring_area_m2(coords: &[f64]) -> f64 {
    const R: f64 = 6_378_137.0; // WGS84 equatorial radius
    let n = coords.len() / 2;
    if n < 3 {
        return 0.0;
    }
    let mut sum = 0.0;
    for i in 0..n {
        let j = (i + 1) % n;
        let lon1 = coords[2 * i].to_radians();
        let lat1 = coords[2 * i + 1].to_radians();
        let lon2 = coords[2 * j].to_radians();
        let lat2 = coords[2 * j + 1].to_radians();
        sum += (lon2 - lon1) * (2.0 + lat1.sin() + lat2.sin());
    }
    (sum * R * R / 2.0).abs()
}

/// Highway-class render rank (0–99); the finest term of `z_order`. Non-highway
/// features get 0. Tunable — mirrors osm2pgsql's road importance ordering.
fn highway_rank(hw: &[u8]) -> i32 {
    match hw {
        b"motorway" | b"motorway_link" => 90,
        b"trunk" | b"trunk_link" => 80,
        b"primary" | b"primary_link" => 70,
        b"secondary" | b"secondary_link" => 60,
        b"tertiary" | b"tertiary_link" => 50,
        b"residential" | b"unclassified" | b"living_street" => 40,
        b"service" => 30,
        b"track" => 20,
        b"path" | b"footway" | b"cycleway" | b"steps" | b"pedestrian" => 10,
        _ => 5,
    }
}

/// osm2pgsql-style render priority for a feature from its tags:
/// `layer*10000 + bridge/tunnel band + highway class rank`.
fn compute_z_order(archive: &Osm, tags: std::ops::Range<u64>) -> i32 {
    let layer = find_tag(archive, tags.clone(), b"layer")
        .and_then(|v| std::str::from_utf8(v).ok())
        .and_then(|s| s.trim().parse::<i32>().ok())
        .unwrap_or(0);
    let class = match find_tag(archive, tags.clone(), b"highway") {
        Some(hw) => highway_rank(hw),
        None => 0,
    };
    let is_bridge = find_tag(archive, tags.clone(), b"bridge").is_some_and(|v| v != b"no");
    let is_tunnel = find_tag(archive, tags.clone(), b"tunnel").is_some_and(|v| v != b"no");
    let band = if is_bridge {
        100
    } else if is_tunnel {
        -100
    } else {
        0
    };
    layer * 10000 + band + class
}

/// Douglas–Peucker simplification of an interleaved `[x0, y0, ...]` polyline,
/// dropping vertices within `tol` (map units, i.e. degrees) of the retained
/// line. Endpoints are always kept, so a closed ring stays closed. `min_pts`
/// guards against collapsing a line/ring below a usable vertex count.
fn simplify_coords(coords: &[f64], tol: f64, min_pts: usize) -> Vec<f64> {
    let n = coords.len() / 2;
    if tol <= 0.0 || n <= min_pts {
        return coords.to_vec();
    }
    let pt = |i: usize| (coords[2 * i], coords[2 * i + 1]);
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let tol2 = tol * tol;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((s, e)) = stack.pop() {
        if e <= s + 1 {
            continue;
        }
        let (ax, ay) = pt(s);
        let (bx, by) = pt(e);
        let (dx, dy) = (bx - ax, by - ay);
        let seg2 = dx * dx + dy * dy;
        let mut max_d2 = 0.0;
        let mut split = s;
        for i in (s + 1)..e {
            let (px, py) = pt(i);
            // Squared distance from point to segment a–b (clamped).
            let d2 = if seg2 == 0.0 {
                (px - ax).powi(2) + (py - ay).powi(2)
            } else {
                let t = (((px - ax) * dx + (py - ay) * dy) / seg2).clamp(0.0, 1.0);
                (px - (ax + t * dx)).powi(2) + (py - (ay + t * dy)).powi(2)
            };
            if d2 > max_d2 {
                max_d2 = d2;
                split = i;
            }
        }
        if max_d2 > tol2 {
            keep[split] = true;
            stack.push((s, split));
            stack.push((split, e));
        }
    }
    if keep.iter().filter(|&&k| k).count() < min_pts {
        return coords.to_vec();
    }
    let mut out = Vec::with_capacity(coords.len());
    for i in 0..n {
        if keep[i] {
            out.push(coords[2 * i]);
            out.push(coords[2 * i + 1]);
        }
    }
    out
}

/// Net area (m²) of a multipolygon: each polygon's exterior minus its holes.
fn multipolygon_area_m2(polygons: &[Vec<Vec<f64>>]) -> f64 {
    polygons
        .iter()
        .map(|poly| {
            let outer = poly.first().map(|r| ring_area_m2(r)).unwrap_or(0.0);
            let holes: f64 = poly.iter().skip(1).map(|r| ring_area_m2(r)).sum();
            (outer - holes).max(0.0)
        })
        .sum()
}

fn materialize_node(
    archive: &Osm,
    idx: usize,
    scale: f64,
    keys: &[&[u8]],
    rel_tags: Option<&std::ops::Range<u64>>,
) -> OwnedFeature {
    let node = &archive.nodes()[idx];
    OwnedFeature {
        osm_id: node_id(archive, idx),
        osm_type: OsmflatOsmType::Node,
        geom_type: OsmflatGeomType::Point,
        is_closed: false,
        z_order: compute_z_order(archive, node.tags()),
        way_area: 0.0,
        coords: vec![node.lon() as f64 / scale, node.lat() as f64 / scale],
        polygons: Vec::new(),
        attrs: collect_attrs(archive, node.tags(), rel_tags, keys),
    }
}

fn materialize_way(
    archive: &Osm,
    idx: usize,
    scale: f64,
    tol: f64,
    keys: &[&[u8]],
    rel_tags: Option<&std::ops::Range<u64>>,
) -> Option<OwnedFeature> {
    let way = &archive.ways()[idx];
    let refs = way.refs();
    let (begin, end) = (refs.start as usize, refs.end as usize);

    let nodes = archive.nodes();
    let nodes_index = archive.nodes_index();
    let mut coords = Vec::new();
    for i in begin..end {
        if let Some(node_idx) = nodes_index[i].value() {
            let node = &nodes[node_idx as usize];
            coords.push(node.lon() as f64 / scale);
            coords.push(node.lat() as f64 / scale);
        }
    }
    // A line string needs at least two vertices.
    if coords.len() < 4 {
        return None;
    }
    let is_closed = end > begin
        && nodes_index[begin].value().is_some()
        && nodes_index[begin].value() == nodes_index[end - 1].value();
    // way_area is computed from the full-resolution ring, before simplification.
    let way_area = if is_closed {
        ring_area_m2(&coords)
    } else {
        0.0
    };
    let coords = simplify_coords(&coords, tol, if is_closed { 4 } else { 2 });

    Some(OwnedFeature {
        osm_id: way_id(archive, idx),
        osm_type: OsmflatOsmType::Way,
        geom_type: OsmflatGeomType::LineString,
        is_closed,
        z_order: compute_z_order(archive, way.tags()),
        way_area,
        coords,
        polygons: Vec::new(),
        attrs: collect_attrs(archive, way.tags(), rel_tags, keys),
    })
}

fn materialize_relation(
    archive: &Osm,
    idx: usize,
    scale: f64,
    tol: f64,
    keys: &[&[u8]],
    mp: Option<osmflat_ext::multipolygon::MultipolygonsQuery>,
) -> Vec<OwnedFeature> {
    let relation = &archive.relations()[idx];
    if !osmflat_ext::multipolygon::is_area_relation(archive, relation) {
        return Vec::new();
    }

    // Prefer the precomputed sidecar (assembled once, offline, by
    // `osmflat-extc --multipolygons`) over live per-query ring stitching --
    // see `osmflat_ext::multipolygon`'s module docs for why live assembly is
    // more fragile than it looks. The sidecar doesn't carry leftover *open*
    // chains (rare, diagnostic-only: a relation whose outer ways don't fully
    // close), so with it active such a relation simply yields no features,
    // rather than the live path's extra unfilled LineString feature.
    let (polygons, open_outer_chains): (Vec<Vec<Vec<f64>>>, Vec<Vec<f64>>) = match mp {
        Some(mp) => {
            let polygons = mp
                .polygons(idx)
                .into_iter()
                .map(|poly| {
                    poly.into_iter()
                        .map(|ring| ring.into_iter().flat_map(|(x, y)| [x, y]).collect())
                        .collect()
                })
                .collect();
            (polygons, Vec::new())
        }
        None => {
            let (polygons, open_outer_chains) =
                osmflat_ext::multipolygon::assemble_multipolygon(archive, idx, scale);
            // Flatten (parent node index + resolved lon/lat) vertices down to
            // the plain interleaved [x0, y0, ...] coords the rest of this
            // function (and the C ABI below) already works in.
            let flatten = |ring: Vec<osmflat_ext::multipolygon::Vertex>| -> Vec<f64> {
                ring.into_iter().flat_map(|v| [v.lon, v.lat]).collect()
            };
            let polygons = polygons
                .into_iter()
                .map(|poly| poly.into_iter().map(flatten).collect())
                .collect();
            let open_outer_chains = open_outer_chains.into_iter().map(flatten).collect();
            (polygons, open_outer_chains)
        }
    };

    if polygons.is_empty() && open_outer_chains.is_empty() {
        let name = find_tag(archive, relation.tags(), b"name")
            .map(|v| String::from_utf8_lossy(v).into_owned())
            .unwrap_or_default();
        eprintln!(
            "osmflat-mapnik-plugin: dropping relation id={:?} name={:?}: outer ways don't form a closed ring",
            relation_id(archive, idx),
            name
        );
        return Vec::new();
    }

    let osm_id = relation_id(archive, idx);
    let z_order = compute_z_order(archive, relation.tags());
    let attrs = collect_attrs(archive, relation.tags(), None, keys);
    let mut features = Vec::new();

    if !polygons.is_empty() {
        // Area from full-resolution rings, then simplify each ring for output.
        let way_area = multipolygon_area_m2(&polygons);
        let polygons: Vec<Vec<Vec<f64>>> = polygons
            .into_iter()
            .map(|poly| {
                poly.into_iter()
                    .map(|ring| simplify_coords(&ring, tol, 4))
                    .collect()
            })
            .collect();
        features.push(OwnedFeature {
            osm_id,
            osm_type: OsmflatOsmType::Relation,
            geom_type: OsmflatGeomType::MultiPolygon,
            is_closed: true,
            z_order,
            way_area,
            coords: Vec::new(),
            polygons,
            attrs: attrs.clone(),
        });
    }

    for chain in open_outer_chains {
        let coords = simplify_coords(&chain, tol, 2);
        if coords.len() < 4 {
            continue;
        }
        features.push(OwnedFeature {
            osm_id,
            osm_type: OsmflatOsmType::Relation,
            geom_type: OsmflatGeomType::LineString,
            is_closed: false,
            z_order,
            way_area: 0.0,
            coords,
            polygons: Vec::new(),
            attrs: attrs.clone(),
        });
    }

    features
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intersect_sorted_u64_basics() {
        assert!(intersect_sorted_u64(&[], &[1, 2]).is_empty());
        assert!(intersect_sorted_u64(&[1, 3, 5], &[2, 4, 6]).is_empty());
        assert_eq!(
            intersect_sorted_u64(&[1, 2, 5, 9], &[2, 5, 8, 9]),
            vec![2, 5, 9]
        );
    }

    use osmflat_extc::test_support::{
        build_ext_archive, build_parent_archive, Fixture, MemberSpec, NodeSpec, RelationSpec,
        TagSpec, WaySpec, COORD_SCALE,
    };

    fn tag(key: &'static str, value: &'static str) -> TagSpec {
        TagSpec::new(key, value)
    }

    /// Two route relations: A (`route=train`, `ref=Borealis`) with a stop node,
    /// two rail ways (one listed twice — must dedup) and a relation member
    /// (must be skipped: no recursion); B (`route=bus`) sharing way 0.
    fn route_fixture() -> Fixture {
        Fixture {
            nodes: vec![
                NodeSpec {
                    lon: -93.2,
                    lat: 44.90,
                    tags: vec![tag("railway", "stop")],
                },
                NodeSpec {
                    lon: -93.1,
                    lat: 44.95,
                    tags: vec![],
                },
                NodeSpec {
                    lon: -93.0,
                    lat: 45.00,
                    tags: vec![],
                },
                NodeSpec {
                    lon: -92.9,
                    lat: 45.05,
                    tags: vec![],
                },
            ],
            ways: vec![
                WaySpec {
                    refs: vec![0, 1, 2],
                    tags: vec![tag("railway", "rail")],
                },
                WaySpec {
                    refs: vec![2, 3],
                    tags: vec![tag("railway", "rail")],
                },
            ],
            relations: vec![
                RelationSpec {
                    bbox: Some((-93.2, 44.90, -92.9, 45.05)),
                    members: vec![
                        MemberSpec::Node(0),
                        MemberSpec::Way(0),
                        MemberSpec::Way(1),
                        MemberSpec::Way(1),
                        MemberSpec::Relation(1),
                    ],
                    tags: vec![
                        tag("type", "route"),
                        tag("route", "train"),
                        tag("ref", "Borealis"),
                    ],
                },
                RelationSpec {
                    bbox: Some((-93.2, 44.90, -93.0, 45.00)),
                    members: vec![MemberSpec::Way(0)],
                    tags: vec![tag("type", "route"), tag("route", "bus")],
                },
            ],
        }
    }

    /// Handle without a sidecar: exercises the full-scan fallback.
    fn plain_handle() -> OsmflatArchive {
        let parent = build_parent_archive(&route_fixture()).unwrap();
        OsmflatArchive {
            kind: ArchiveKind::Plain(parent),
            coord_scale: COORD_SCALE as f64,
        }
    }

    /// Handle with a taginfo sidecar: exercises the inverted-index path.
    fn ext_handle() -> OsmflatArchive {
        let parent = build_parent_archive(&route_fixture()).unwrap();
        let opts = osmflat_extc::BuildOptions {
            taginfo: true,
            ..Default::default()
        };
        OsmflatArchive {
            kind: ArchiveKind::Ext(build_ext_archive(parent, &opts).unwrap()),
            coord_scale: COORD_SCALE as f64,
        }
    }

    /// Index of the (single) relation carrying `key=value`, after the
    /// builder's spatial reordering.
    fn relation_with(archive: &Osm, key: &[u8], value: &[u8]) -> u64 {
        (0..archive.relations().len())
            .find(|&i| find_tag(archive, archive.relations()[i].tags(), key) == Some(value))
            .unwrap() as u64
    }

    #[test]
    fn relations_matching_all_ands_terms_on_both_paths() {
        for handle in [plain_handle(), ext_handle()] {
            let archive = handle.osm();
            let train = relation_with(archive, b"route", b"train");
            let bus = relation_with(archive, b"route", b"bus");

            let filters = vec![
                (b"route".as_slice(), Some(b"train".as_slice())),
                (b"ref".as_slice(), Some(b"Borealis".as_slice())),
            ];
            assert_eq!(relations_matching_all(&handle, &filters), vec![train]);

            // AND, not OR: one non-matching term empties the result.
            let filters = vec![
                (b"route".as_slice(), Some(b"train".as_slice())),
                (b"ref".as_slice(), Some(b"Nope".as_slice())),
            ];
            assert!(relations_matching_all(&handle, &filters).is_empty());

            // Wildcard term: any relation carrying the key.
            let filters = vec![(b"route".as_slice(), None)];
            let mut expected = vec![train, bus];
            expected.sort_unstable();
            assert_eq!(relations_matching_all(&handle, &filters), expected);

            assert!(relations_matching_all(&handle, &[]).is_empty());
        }
    }

    #[test]
    fn expand_member_pairs_dedups_and_skips_relation_members() {
        let handle = plain_handle();
        let archive = handle.osm();
        let train = relation_with(archive, b"route", b"train");

        let (node_pairs, way_pairs) = expand_member_pairs(archive, &[train], true, true);

        assert_eq!(node_pairs.len(), 1);
        assert_eq!(way_pairs.len(), 2); // duplicate way deduped, relation member dropped
        assert!(node_pairs
            .iter()
            .chain(&way_pairs)
            .all(|&(_, r)| r == train));
        assert!(way_pairs[0].0 < way_pairs[1].0); // ascending (spatial) order

        // want_* gates each list independently.
        let (no_nodes, ways_only) = expand_member_pairs(archive, &[train], false, true);
        assert!(no_nodes.is_empty());
        assert_eq!(ways_only, way_pairs);
    }

    #[test]
    fn rel_prefixed_keys_resolve_from_parent_relation() {
        let handle = plain_handle();
        let archive = handle.osm();
        let train = relation_with(archive, b"route", b"train");
        let (_, way_pairs) = expand_member_pairs(archive, &[train], false, true);
        let (w, r) = way_pairs[0];

        let rel_tags = archive.relations()[r as usize].tags();
        let keys = vec![
            b"railway".as_slice(),
            b"ref".as_slice(),
            b"rel_ref".as_slice(),
            b"rel_route".as_slice(),
        ];
        let feat = materialize_way(
            archive,
            w as usize,
            handle.coord_scale,
            0.0,
            &keys,
            Some(&rel_tags),
        )
        .unwrap();

        assert_eq!(feat.attrs[0].as_deref(), Some(b"rail".as_slice())); // own tag
        assert_eq!(feat.attrs[1], None); // the way itself has no `ref`
        assert_eq!(feat.attrs[2].as_deref(), Some(b"Borealis".as_slice()));
        assert_eq!(feat.attrs[3].as_deref(), Some(b"train".as_slice()));

        // Without a parent relation, `rel_*` is just a literal (absent) tag.
        let plain =
            materialize_way(archive, w as usize, handle.coord_scale, 0.0, &keys, None).unwrap();
        assert_eq!(plain.attrs[2], None);
    }
}

/// Frees a feature set.
///
/// # Safety
/// `fs` must be from `osmflat_query` (or null) and unused afterward.
#[no_mangle]
pub unsafe extern "C" fn osmflat_featureset_free(fs: *mut OsmflatFeatureSet) {
    if !fs.is_null() {
        drop(Box::from_raw(fs));
    }
}

/// Advances to the next feature. Returns true if a feature is now current
/// (readable via the getters), false when exhausted.
///
/// # Safety
/// `fs` must be a valid handle from `osmflat_query`.
#[no_mangle]
pub unsafe extern "C" fn osmflat_featureset_next(fs: *mut OsmflatFeatureSet) -> bool {
    let Some(fs) = fs.as_mut() else {
        return false;
    };
    if fs.pos >= fs.features.len() {
        return false;
    }
    fs.pos += 1;
    true
}

/// Whether the current feature has a real OSM id (false → render `osm_id` as
/// null). When true, `*out` receives the id.
///
/// # Safety
/// `fs` must be valid; `out` must be valid for a write.
#[no_mangle]
pub unsafe extern "C" fn osmflat_feature_osm_id(
    fs: *const OsmflatFeatureSet,
    out: *mut u64,
) -> bool {
    match fs
        .as_ref()
        .and_then(|fs| fs.current())
        .and_then(|f| f.osm_id)
    {
        Some(id) => {
            out.write(id);
            true
        }
        None => false,
    }
}

/// OSM primitive type of the current feature.
///
/// # Safety
/// `fs` must be a valid handle from `osmflat_query`.
#[no_mangle]
pub unsafe extern "C" fn osmflat_feature_osm_type(fs: *const OsmflatFeatureSet) -> OsmflatOsmType {
    match fs.as_ref().and_then(|fs| fs.current()).map(|f| &f.osm_type) {
        Some(OsmflatOsmType::Way) => OsmflatOsmType::Way,
        Some(OsmflatOsmType::Relation) => OsmflatOsmType::Relation,
        _ => OsmflatOsmType::Node,
    }
}

/// Whether the current feature's geometry is a closed ring.
///
/// # Safety
/// `fs` must be a valid handle from `osmflat_query`.
#[no_mangle]
pub unsafe extern "C" fn osmflat_feature_is_closed(fs: *const OsmflatFeatureSet) -> bool {
    fs.as_ref()
        .and_then(|fs| fs.current())
        .map(|f| f.is_closed)
        .unwrap_or(false)
}

/// Enclosed area of the current feature in square meters (spherical); 0 for
/// points and open ways.
///
/// # Safety
/// `fs` must be a valid handle from `osmflat_query`.
#[no_mangle]
pub unsafe extern "C" fn osmflat_feature_way_area(fs: *const OsmflatFeatureSet) -> f64 {
    fs.as_ref()
        .and_then(|fs| fs.current())
        .map(|f| f.way_area)
        .unwrap_or(0.0)
}

/// osm2pgsql-style render priority of the current feature.
///
/// # Safety
/// `fs` must be a valid handle from `osmflat_query`.
#[no_mangle]
pub unsafe extern "C" fn osmflat_feature_z_order(fs: *const OsmflatFeatureSet) -> i32 {
    fs.as_ref()
        .and_then(|fs| fs.current())
        .map(|f| f.z_order)
        .unwrap_or(0)
}

/// Geometry type of the current feature.
///
/// # Safety
/// `fs` must be a valid handle from `osmflat_query`.
#[no_mangle]
pub unsafe extern "C" fn osmflat_feature_geom_type(
    fs: *const OsmflatFeatureSet,
) -> OsmflatGeomType {
    match fs
        .as_ref()
        .and_then(|fs| fs.current())
        .map(|f| &f.geom_type)
    {
        Some(OsmflatGeomType::LineString) => OsmflatGeomType::LineString,
        Some(OsmflatGeomType::Polygon) => OsmflatGeomType::Polygon,
        Some(OsmflatGeomType::MultiPolygon) => OsmflatGeomType::MultiPolygon,
        _ => OsmflatGeomType::Point,
    }
}

/// Number of polygons in a `MultiPolygon` feature (0 for point/line).
///
/// # Safety
/// `fs` must be a valid handle from `osmflat_query`.
#[no_mangle]
pub unsafe extern "C" fn osmflat_feature_num_polygons(fs: *const OsmflatFeatureSet) -> usize {
    fs.as_ref()
        .and_then(|fs| fs.current())
        .map(|f| f.polygons.len())
        .unwrap_or(0)
}

/// Number of rings in polygon `p` (ring 0 is the exterior, rings 1.. are holes).
///
/// # Safety
/// `fs` must be a valid handle from `osmflat_query`.
#[no_mangle]
pub unsafe extern "C" fn osmflat_feature_polygon_num_rings(
    fs: *const OsmflatFeatureSet,
    p: usize,
) -> usize {
    fs.as_ref()
        .and_then(|fs| fs.current())
        .and_then(|f| f.polygons.get(p))
        .map(|poly| poly.len())
        .unwrap_or(0)
}

/// Number of `(x, y)` vertices in ring `r` of polygon `p`.
///
/// # Safety
/// `fs` must be a valid handle from `osmflat_query`.
#[no_mangle]
pub unsafe extern "C" fn osmflat_feature_ring_num_coords(
    fs: *const OsmflatFeatureSet,
    p: usize,
    r: usize,
) -> usize {
    fs.as_ref()
        .and_then(|fs| fs.current())
        .and_then(|f| f.polygons.get(p))
        .and_then(|poly| poly.get(r))
        .map(|ring| ring.len() / 2)
        .unwrap_or(0)
}

/// Pointer to ring `r` of polygon `p` as interleaved `[x0, y0, ...]` (length
/// `2 * osmflat_feature_ring_num_coords`). Valid until the next advance / free.
///
/// # Safety
/// `fs` must be a valid handle from `osmflat_query`.
#[no_mangle]
pub unsafe extern "C" fn osmflat_feature_ring_coords(
    fs: *const OsmflatFeatureSet,
    p: usize,
    r: usize,
) -> *const f64 {
    fs.as_ref()
        .and_then(|fs| fs.current())
        .and_then(|f| f.polygons.get(p))
        .and_then(|poly| poly.get(r))
        .map(|ring| ring.as_ptr())
        .unwrap_or(std::ptr::null())
}

/// Number of `(x, y)` vertices in the current feature's geometry.
///
/// # Safety
/// `fs` must be a valid handle from `osmflat_query`.
#[no_mangle]
pub unsafe extern "C" fn osmflat_feature_num_coords(fs: *const OsmflatFeatureSet) -> usize {
    fs.as_ref()
        .and_then(|fs| fs.current())
        .map(|f| f.coords.len() / 2)
        .unwrap_or(0)
}

/// Pointer to the current feature's interleaved `[x0, y0, ...]` coords (length
/// `2 * osmflat_feature_num_coords`). Valid until the next advance / free.
///
/// # Safety
/// `fs` must be a valid handle from `osmflat_query`.
#[no_mangle]
pub unsafe extern "C" fn osmflat_feature_coords(fs: *const OsmflatFeatureSet) -> *const f64 {
    fs.as_ref()
        .and_then(|fs| fs.current())
        .map(|f| f.coords.as_ptr())
        .unwrap_or(std::ptr::null())
}

/// Value of the current feature's `i`th requested attribute (aligned to the
/// `keys` passed to `osmflat_query`). `present == false` for an absent tag or
/// out-of-range `i`. Bytes valid until the next advance / free.
///
/// # Safety
/// `fs` must be a valid handle from `osmflat_query`.
#[no_mangle]
pub unsafe extern "C" fn osmflat_feature_attr(
    fs: *const OsmflatFeatureSet,
    i: usize,
) -> OsmflatValue {
    match fs
        .as_ref()
        .and_then(|fs| fs.current())
        .and_then(|f| f.attrs.get(i))
    {
        Some(Some(bytes)) => OsmflatValue {
            present: true,
            ptr: bytes.as_ptr(),
            len: bytes.len(),
        },
        _ => OsmflatValue {
            present: false,
            ptr: std::ptr::null(),
            len: 0,
        },
    }
}
