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

/// A borrowed key handed *in* from C++ (a name from `query::property_names()`),
/// as raw UTF-8 bytes without a trailing NUL.
#[repr(C)]
pub struct OsmflatStrRef {
    pub ptr: *const u8,
    pub len: usize,
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

/// Opaque archive handle. Owns the memory-mapped `Osm` archive.
pub struct OsmflatArchive {
    archive: Osm,
    coord_scale: f64,
}

struct OwnedFeature {
    /// Real OSM id, or `None` if the archive has no `ids` sub-archive.
    osm_id: Option<u64>,
    osm_type: OsmflatOsmType,
    geom_type: OsmflatGeomType,
    is_closed: bool,
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

/// Opens an osmflat archive directory. Returns null on failure. Free with
/// `osmflat_archive_free`.
///
/// # Safety
/// `path` must be a valid, NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn osmflat_archive_open(path: *const c_char) -> *mut OsmflatArchive {
    if path.is_null() {
        return std::ptr::null_mut();
    }
    let path = match CStr::from_ptr(path).to_str() {
        Ok(p) => p,
        Err(_) => return std::ptr::null_mut(),
    };

    let archive = match Osm::open(FileResourceStorage::new(path)) {
        Ok(a) => a,
        Err(_) => return std::ptr::null_mut(),
    };
    let coord_scale = archive.header().coord_scale() as f64;

    Box::into_raw(Box::new(OsmflatArchive {
        archive,
        coord_scale,
    }))
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
    let header = archive.archive.header();
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
) -> *mut OsmflatFeatureSet {
    let Some(handle) = archive.as_ref() else {
        return std::ptr::null_mut();
    };
    let archive = &handle.archive;
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

    let mut features = Vec::new();

    if include_nodes {
        let node_base = archive.nodes().as_ptr();
        for node in find_nodes_by_bounding_box(archive, min_x, min_y, max_x, max_y) {
            let idx = (node as *const Node).offset_from(node_base) as usize;
            let x = node.lon() as f64 / scale;
            let y = node.lat() as f64 / scale;
            features.push(OwnedFeature {
                osm_id: node_id(archive, idx),
                osm_type: OsmflatOsmType::Node,
                geom_type: OsmflatGeomType::Point,
                is_closed: false,
                coords: vec![x, y],
                polygons: Vec::new(),
                attrs: collect_attrs(archive, node.tags(), &key_refs),
            });
        }
    }

    if include_ways {
        let nodes = archive.nodes();
        let nodes_index = archive.nodes_index();
        let way_base = archive.ways().as_ptr();
        for way in find_ways_by_bounding_box(archive, min_x, min_y, max_x, max_y) {
            let refs = way.refs();
            let (begin, end) = (refs.start as usize, refs.end as usize);

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
                continue;
            }

            // Closedness is a geometric fact: the first and last node ref
            // resolve to the same node.
            let is_closed = end > begin
                && nodes_index[begin].value().is_some()
                && nodes_index[begin].value() == nodes_index[end - 1].value();

            let idx = (way as *const Way).offset_from(way_base) as usize;
            features.push(OwnedFeature {
                osm_id: way_id(archive, idx),
                osm_type: OsmflatOsmType::Way,
                geom_type: OsmflatGeomType::LineString,
                is_closed,
                coords,
                polygons: Vec::new(),
                attrs: collect_attrs(archive, way.tags(), &key_refs),
            });
        }
    }

    if include_relations {
        let rel_base = archive.relations().as_ptr();
        for relation in find_relations_by_bounding_box(archive, min_x, min_y, max_x, max_y) {
            // Only area relations become geometry; routes etc. are skipped.
            if !is_area_relation(archive, relation) {
                continue;
            }
            let idx = (relation as *const Relation).offset_from(rel_base) as usize;
            let polygons = assemble_multipolygon(archive, idx, scale);
            if polygons.is_empty() {
                continue;
            }
            features.push(OwnedFeature {
                osm_id: relation_id(archive, idx),
                osm_type: OsmflatOsmType::Relation,
                geom_type: OsmflatGeomType::MultiPolygon,
                is_closed: true,
                coords: Vec::new(),
                polygons,
                attrs: collect_attrs(archive, relation.tags(), &key_refs),
            });
        }
    }

    Box::into_raw(Box::new(OsmflatFeatureSet { features, pos: 0 }))
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

/// Stitch open/closed member segments (each a node-index sequence) into closed
/// rings by matching shared endpoints. Only rings that close are returned;
/// unclosable leftovers (e.g. a member way outside a clipped extract) are dropped.
fn assemble_rings(mut segments: Vec<Vec<u64>>) -> Vec<Vec<u64>> {
    let mut rings = Vec::new();
    while let Some(mut ring) = segments.pop() {
        loop {
            if ring.len() > 1 && ring.first() == ring.last() {
                rings.push(ring);
                break;
            }
            let end = *ring.last().unwrap();
            // Find a remaining segment sharing this open endpoint.
            let next = segments.iter().position(|s| {
                s.first() == Some(&end) || s.last() == Some(&end)
            });
            match next {
                Some(i) => {
                    let mut seg = segments.remove(i);
                    if seg.last() == Some(&end) {
                        seg.reverse();
                    }
                    // seg now starts at `end`; append the rest, skipping the shared node.
                    ring.extend_from_slice(&seg[1..]);
                }
                None => break, // cannot close; drop this partial ring
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

    let mut outer_segs: Vec<Vec<u64>> = Vec::new();
    let mut inner_segs: Vec<Vec<u64>> = Vec::new();
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
            inner_segs.push(seg);
        } else {
            outer_segs.push(seg);
        }
    }

    let outer_rings = assemble_rings(outer_segs);
    let inner_rings = assemble_rings(inner_segs);

    let nodes = archive.nodes();
    let to_coords = |ring: &[u64]| -> Vec<(f64, f64)> {
        ring.iter()
            .map(|&n| {
                let node = &nodes[n as usize];
                (node.lon() as f64 / scale, node.lat() as f64 / scale)
            })
            .collect()
    };
    let flatten = |ring: &[(f64, f64)]| -> Vec<f64> {
        ring.iter().flat_map(|&(x, y)| [x, y]).collect()
    };

    let outers: Vec<Vec<(f64, f64)>> = outer_rings.iter().map(|r| to_coords(r)).collect();
    if outers.is_empty() {
        return Vec::new();
    }

    // One polygon per exterior ring; assign each hole to the exterior that
    // contains its first vertex.
    let mut polygons: Vec<Vec<Vec<f64>>> = outers.iter().map(|o| vec![flatten(o)]).collect();
    for inner in &inner_rings {
        let inner_coords = to_coords(inner);
        let Some(&first) = inner_coords.first() else {
            continue;
        };
        if let Some(oi) = outers.iter().position(|o| point_in_ring(first, o)) {
            polygons[oi].push(flatten(&inner_coords));
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
