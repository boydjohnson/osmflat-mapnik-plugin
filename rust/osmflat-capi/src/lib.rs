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
    find_nodes_by_bounding_box, find_tag, find_ways_by_bounding_box, node_id, way_id,
    FileResourceStorage, Node, Osm, Way,
};

/// Geometry kind of a materialized feature. `Polygon` is reserved for phase-2
/// multipolygon-relation support and is not yet emitted.
#[repr(u32)]
pub enum OsmflatGeomType {
    Point = 1,
    LineString = 2,
    Polygon = 3,
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
    /// Interleaved `[x0, y0, x1, y1, ...]` in degrees (EPSG:4326).
    coords: Vec<f64>,
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
/// are degrees (lon = x, lat = y). `include_nodes`/`include_ways` select which
/// primitives to emit (a performance filter, not semantics). `keys`/`num_keys`
/// are the tag names to materialize (from `query::property_names()`, synthetic
/// names already stripped by the caller). Free with `osmflat_featureset_free`.
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
                attrs: collect_attrs(archive, way.tags(), &key_refs),
            });
        }
    }

    Box::into_raw(Box::new(OsmflatFeatureSet { features, pos: 0 }))
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
        _ => OsmflatGeomType::Point,
    }
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
