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
    find_nodes_by_bounding_box, find_relations_by_bounding_box, find_tag, find_ways_by_bounding_box,
    node_id, relation_id, way_id, FileResourceStorage, Node, Osm, Relation, RelationMembersRef, Way,
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

/// Look up the requested tag values for `range`, aligned to `keys`.
fn collect_attrs(
    archive: &Osm,
    range: std::ops::Range<u64>,
    keys: &[&[u8]],
) -> Vec<Option<Vec<u8>>> {
    keys.iter()
        .map(|key| find_tag(archive, range.clone(), key).map(|v| v.to_vec()))
        .collect()
}

/// Runs a bounding-box query and returns an owned feature set. `min_*`/`max_*`
/// are degrees (lon = x, lat = y). `include_nodes`/`include_ways`/
/// `include_relations` select which primitives to emit (a performance filter,
/// not semantics); relations are emitted only for `type=multipolygon`/`boundary`
/// as assembled multipolygons. `keys`/`num_keys` are the tag names to
/// materialize (from `query::property_names()`, synthetic names already stripped
/// by the caller). Free with `osmflat_featureset_free`.
///
/// # Safety
/// `archive` must be valid; `keys` must point to `num_keys` valid `OsmflatStrRef`
/// whose byte ranges are valid for the duration of the call.
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

    // Parse the tag prefilter: each `(key, Some(value))` is `key=value`; a
    // zero-length value means `key=*` (any value of the key).
    let filter_refs: Vec<(&[u8], Option<&[u8]>)> = if num_filters == 0 {
        Vec::new()
    } else {
        slice::from_raw_parts(filters, num_filters)
            .iter()
            .filter_map(|f| {
                let key = str_ref(&f.key)?;
                let val = str_ref(&f.val);
                Some((key, val.filter(|v| !v.is_empty())))
            })
            .collect()
    };

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
    if include_nodes {
        match candidate_indices(handle, &filter_refs, bbox, Prim::Node) {
            Some(indices) => features.extend(
                indices
                    .into_iter()
                    .map(|i| materialize_node(archive, i as usize, scale, &key_refs)),
            ),
            None => {
                let base = archive.nodes().as_ptr();
                for node in find_nodes_by_bounding_box(archive, min_x, min_y, max_x, max_y) {
                    let idx = (node as *const Node).offset_from(base) as usize;
                    features.push(materialize_node(archive, idx, scale, &key_refs));
                }
            }
        }
    }

    if include_ways {
        match candidate_indices(handle, &filter_refs, bbox, Prim::Way) {
            Some(indices) => features.extend(
                indices
                    .into_iter()
                    .filter_map(|i| materialize_way(archive, i as usize, scale, simplify_tolerance, &key_refs)),
            ),
            None => {
                let base = archive.ways().as_ptr();
                for way in find_ways_by_bounding_box(archive, min_x, min_y, max_x, max_y) {
                    let idx = (way as *const Way).offset_from(base) as usize;
                    if let Some(f) = materialize_way(archive, idx, scale, simplify_tolerance, &key_refs) {
                        features.push(f);
                    }
                }
            }
        }
    }

    if include_relations {
        match candidate_indices(handle, &filter_refs, bbox, Prim::Relation) {
            Some(indices) => features.extend(
                indices
                    .into_iter()
                    .filter_map(|i| materialize_relation(archive, i as usize, scale, simplify_tolerance, &key_refs)),
            ),
            None => {
                let base = archive.relations().as_ptr();
                for relation in find_relations_by_bounding_box(archive, min_x, min_y, max_x, max_y) {
                    let idx = (relation as *const Relation).offset_from(base) as usize;
                    if let Some(f) = materialize_relation(archive, idx, scale, simplify_tolerance, &key_refs) {
                        features.push(f);
                    }
                }
            }
        }
    }

    // Apply the requested draw order (mapnik renders in the returned order).
    match order {
        OsmflatOrder::ZOrder => features.sort_by_key(|f| f.z_order),
        OsmflatOrder::WayArea => {
            features.sort_by(|a, b| b.way_area.partial_cmp(&a.way_area).unwrap_or(std::cmp::Ordering::Equal))
        }
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

fn materialize_node(archive: &Osm, idx: usize, scale: f64, keys: &[&[u8]]) -> OwnedFeature {
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
        attrs: collect_attrs(archive, node.tags(), keys),
    }
}

fn materialize_way(
    archive: &Osm,
    idx: usize,
    scale: f64,
    tol: f64,
    keys: &[&[u8]],
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
    let way_area = if is_closed { ring_area_m2(&coords) } else { 0.0 };
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
        attrs: collect_attrs(archive, way.tags(), keys),
    })
}

fn materialize_relation(
    archive: &Osm,
    idx: usize,
    scale: f64,
    tol: f64,
    keys: &[&[u8]],
) -> Option<OwnedFeature> {
    let relation = &archive.relations()[idx];
    if !is_area_relation(archive, relation) {
        return None;
    }
    let polygons = assemble_multipolygon(archive, idx, scale);
    if polygons.is_empty() {
        let name = find_tag(archive, relation.tags(), b"name")
            .map(|v| String::from_utf8_lossy(v).into_owned())
            .unwrap_or_default();
        eprintln!(
            "osmflat-mapnik-plugin: dropping relation id={:?} name={:?}: outer ways don't form a closed ring",
            relation_id(archive, idx),
            name
        );
        return None;
    }
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
    Some(OwnedFeature {
        osm_id: relation_id(archive, idx),
        osm_type: OsmflatOsmType::Relation,
        geom_type: OsmflatGeomType::MultiPolygon,
        is_closed: true,
        z_order: compute_z_order(archive, relation.tags()),
        way_area,
        coords: Vec::new(),
        polygons,
        attrs: collect_attrs(archive, relation.tags(), keys),
    })
}

/// True if the relation is an area type whose member ways enclose polygons.
fn is_area_relation(archive: &Osm, relation: &Relation) -> bool {
    match find_tag(archive, relation.tags(), b"type") {
        Some(v) => v == b"multipolygon" || v == b"boundary",
        None => false,
    }
}

/// Resolve a way's node-index sequence (dropping unresolved refs).
fn way_node_indices(archive: &Osm, way: &Way) -> Vec<u64> {
    let nodes_index = archive.nodes_index();
    let refs = way.refs();
    (refs.start as usize..refs.end as usize)
        .filter_map(|i| nodes_index[i].value())
        .collect()
}

/// Endpoints within this distance are treated as the same junction when
/// stitching ring segments. Chosen to bridge duplicate-node seams seen in
/// real extracts (tens to a couple hundred meters) while staying well below
/// the gap left by genuinely missing boundary members (800m+).
const RING_SNAP_TOLERANCE_M: f64 = 250.0;

/// Approximate great-circle distance in meters between two `(lon, lat)`
/// points in degrees. Equirectangular approximation; adequate at the scale
/// of ring-closing gaps (tens to hundreds of meters).
fn dist_m(a: (f64, f64), b: (f64, f64)) -> f64 {
    const R: f64 = 6_378_137.0;
    let (lon1, lat1) = (a.0.to_radians(), a.1.to_radians());
    let (lon2, lat2) = (b.0.to_radians(), b.1.to_radians());
    let x = (lon2 - lon1) * ((lat1 + lat2) / 2.0).cos();
    let y = lat2 - lat1;
    R * (x * x + y * y).sqrt()
}

/// Endpoints closer than this are treated as identical (floating-point
/// round-trip noise only) — used to detect a way that is already closed on
/// its own, which must not be confused with the much larger snap tolerance
/// used for bridging duplicate-node seams between *different* ways.
const EXACT_EPS_M: f64 = 0.01;

/// Stitch open/closed member segments (each a coordinate sequence) into
/// closed rings by matching endpoints, exactly or (once at least one seam
/// between two distinct ways has been stitched) within
/// `RING_SNAP_TOLERANCE_M` (real-world extracts sometimes encode the same
/// junction as two distinct, near-coincident nodes). A lone way is only
/// accepted as its own ring if its ends are exactly coincident — otherwise a
/// short way whose two ends simply happen to be near each other would be
/// misread as a closed area. Leftovers with a gap too large to bridge (e.g.
/// a member way missing from a clipped extract) are dropped.
fn assemble_rings(mut segments: Vec<Vec<(f64, f64)>>) -> Vec<Vec<(f64, f64)>> {
    let mut rings = Vec::new();
    while let Some(mut ring) = segments.pop() {
        let mut stitched = 0u32;
        loop {
            let close_dist = dist_m(*ring.first().unwrap(), *ring.last().unwrap());
            let closed = ring.len() > 1
                && if stitched > 0 {
                    close_dist < RING_SNAP_TOLERANCE_M
                } else {
                    close_dist < EXACT_EPS_M
                };
            if closed {
                rings.push(ring);
                break;
            }
            let end = *ring.last().unwrap();
            // Find a remaining segment whose near end is close to this open endpoint.
            let next = segments.iter().position(|s| {
                dist_m(*s.first().unwrap(), end) < RING_SNAP_TOLERANCE_M
                    || dist_m(*s.last().unwrap(), end) < RING_SNAP_TOLERANCE_M
            });
            match next {
                Some(i) => {
                    let mut seg = segments.remove(i);
                    if dist_m(*seg.last().unwrap(), end) < dist_m(*seg.first().unwrap(), end) {
                        seg.reverse();
                    }
                    // seg now starts near `end`; append the rest, skipping its matched vertex.
                    ring.extend_from_slice(&seg[1..]);
                    stitched += 1;
                }
                None => break, // gap too large to bridge; drop this partial ring
            }
        }
    }
    rings
}

/// Ray-casting point-in-polygon test against a ring of `(x, y)` vertices.
fn point_in_ring(pt: (f64, f64), ring: &[(f64, f64)]) -> bool {
    let (px, py) = pt;
    let mut inside = false;
    let n = ring.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = ring[i];
        let (xj, yj) = ring[j];
        if (yi > py) != (yj > py) && px < (xj - xi) * (py - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Assemble a `type=multipolygon`/`boundary` relation into polygons, each an
/// exterior ring followed by the holes it contains. Returns
/// `polygons[p][r]` = interleaved `[x0, y0, ...]` coords for ring `r`.
fn assemble_multipolygon(archive: &Osm, rel_idx: usize, scale: f64) -> Vec<Vec<Vec<f64>>> {
    let members = archive.relation_members();
    let ways = archive.ways();
    let strings = archive.stringtable();
    let nodes = archive.nodes();

    let to_coords = |seg: &[u64]| -> Vec<(f64, f64)> {
        seg.iter()
            .map(|&n| {
                let node = &nodes[n as usize];
                (node.lon() as f64 / scale, node.lat() as f64 / scale)
            })
            .collect()
    };

    let mut outer_segs: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut inner_segs: Vec<Vec<(f64, f64)>> = Vec::new();
    for member in members.at(rel_idx) {
        let RelationMembersRef::WayMember(wm) = member else {
            continue;
        };
        let Some(way_idx) = wm.way_idx() else { continue };
        let seg = way_node_indices(archive, &ways[way_idx as usize]);
        if seg.len() < 2 {
            continue;
        }
        // Role "inner" carves holes; everything else (outer, empty) is exterior.
        if strings.substring_raw(wm.role_idx() as usize) == b"inner" {
            inner_segs.push(to_coords(&seg));
        } else {
            outer_segs.push(to_coords(&seg));
        }
    }

    let outers = assemble_rings(outer_segs);
    let inner_rings = assemble_rings(inner_segs);

    let flatten = |ring: &[(f64, f64)]| -> Vec<f64> {
        ring.iter().flat_map(|&(x, y)| [x, y]).collect()
    };

    if outers.is_empty() {
        return Vec::new();
    }

    // One polygon per exterior ring; assign each hole to the exterior that
    // contains its first vertex.
    let mut polygons: Vec<Vec<Vec<f64>>> = outers.iter().map(|o| vec![flatten(o)]).collect();
    for inner_coords in &inner_rings {
        let Some(&first) = inner_coords.first() else {
            continue;
        };
        if let Some(oi) = outers.iter().position(|o| point_in_ring(first, o)) {
            polygons[oi].push(flatten(inner_coords));
        }
        // A hole with no containing exterior is dropped (malformed relation).
    }

    polygons
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
