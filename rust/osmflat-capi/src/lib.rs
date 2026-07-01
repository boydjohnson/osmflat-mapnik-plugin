//! C ABI over the `osmflat` spatial-query API, consumed by the C++ mapnik
//! datasource plugin in `../../src`.
//!
//! Design: an opaque archive handle (`OsmflatArchive`) is opened once and held
//! by the datasource. Each mapnik `features(query)` call runs a bounding-box
//! query and materializes the matching features into an owned
//! `OsmflatFeatureSet`, which the C++ `Featureset::next()` then pulls one at a
//! time. Materializing up front keeps the FFI free of borrows into the archive:
//! every pointer handed to C++ is owned by the feature set and stays valid
//! until the next `osmflat_featureset_next` or `osmflat_featureset_free`.

use std::ffi::CStr;
use std::os::raw::c_char;

use osmflat::{
    find_nodes_by_bounding_box, find_ways_by_bounding_box, iter_tags, FileResourceStorage, Osm,
};

/// Geometry kind of a materialized feature. Matches mapnik's point/line
/// geometries; polygons (from relations) are intentionally not yet emitted.
#[repr(u32)]
pub enum OsmflatGeomType {
    Point = 1,
    LineString = 2,
}

/// A borrowed, non-owning view of bytes handed back to C++ for tag keys and
/// values. Valid only for the lifetime of the current feature (i.e. until the
/// next `osmflat_featureset_next` call or the feature set is freed).
#[repr(C)]
pub struct OsmflatBytes {
    pub ptr: *const u8,
    pub len: usize,
}

/// Opaque archive handle. Owns the memory-mapped `Osm` archive.
pub struct OsmflatArchive {
    archive: Osm,
    coord_scale: f64,
}

struct OwnedFeature {
    id: u64,
    geom_type: OsmflatGeomType,
    /// Interleaved `[x0, y0, x1, y1, ...]` in degrees (EPSG:4326).
    coords: Vec<f64>,
    /// Tag key/value pairs as raw UTF-8 bytes (no trailing NUL).
    tags: Vec<(Vec<u8>, Vec<u8>)>,
}

/// Opaque, owned result of one bounding-box query. The C++ side pulls features
/// via `osmflat_featureset_next` and the per-feature getters.
pub struct OsmflatFeatureSet {
    features: Vec<OwnedFeature>,
    /// Index of the *next* feature to yield. After a successful
    /// `osmflat_featureset_next`, the current feature is `features[pos - 1]`.
    pos: usize,
}

impl OsmflatFeatureSet {
    fn current(&self) -> Option<&OwnedFeature> {
        self.pos.checked_sub(1).and_then(|i| self.features.get(i))
    }
}

/// Opens an osmflat archive directory. Returns null on failure (bad path,
/// missing/corrupt archive). The returned handle must be freed with
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

    let storage = FileResourceStorage::new(path);
    let archive = match Osm::open(storage) {
        Ok(a) => a,
        Err(_) => return std::ptr::null_mut(),
    };
    let coord_scale = archive.header().coord_scale() as f64;

    Box::into_raw(Box::new(OsmflatArchive {
        archive,
        coord_scale,
    }))
}

/// Frees an archive handle from `osmflat_archive_open`.
///
/// # Safety
/// `archive` must be a pointer from `osmflat_archive_open` (or null) and must
/// not be used afterward.
#[no_mangle]
pub unsafe extern "C" fn osmflat_archive_free(archive: *mut OsmflatArchive) {
    if !archive.is_null() {
        drop(Box::from_raw(archive));
    }
}

/// Writes the archive's bounding box (degrees) into the out pointers. Returns
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

fn collect_tags(archive: &Osm, range: std::ops::Range<u64>) -> Vec<(Vec<u8>, Vec<u8>)> {
    iter_tags(archive, range)
        .map(|(k, v)| (k.to_vec(), v.to_vec()))
        .collect()
}

/// Runs a bounding-box query and returns an owned feature set. `min_*`/`max_*`
/// are in degrees (lon = x, lat = y). `include_nodes` emits matching nodes as
/// points; `include_ways` emits matching ways as line strings. The result must
/// be freed with `osmflat_featureset_free`.
///
/// # Safety
/// `archive` must be a valid handle from `osmflat_archive_open`.
#[no_mangle]
pub unsafe extern "C" fn osmflat_query(
    archive: *const OsmflatArchive,
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
    include_nodes: bool,
    include_ways: bool,
) -> *mut OsmflatFeatureSet {
    let Some(handle) = archive.as_ref() else {
        return std::ptr::null_mut();
    };
    let archive = &handle.archive;
    let scale = handle.coord_scale;

    let mut features = Vec::new();

    if include_nodes {
        for node in find_nodes_by_bounding_box(archive, min_x, min_y, max_x, max_y) {
            let x = node.lon() as f64 / scale;
            let y = node.lat() as f64 / scale;
            features.push(OwnedFeature {
                id: 0,
                geom_type: OsmflatGeomType::Point,
                coords: vec![x, y],
                tags: collect_tags(archive, node.tags()),
            });
        }
    }

    if include_ways {
        let nodes = archive.nodes();
        let nodes_index = archive.nodes_index();
        for way in find_ways_by_bounding_box(archive, min_x, min_y, max_x, max_y) {
            let mut coords = Vec::new();
            for i in way.refs() {
                if let Some(node_idx) = nodes_index[i as usize].value() {
                    let node = &nodes[node_idx as usize];
                    coords.push(node.lon() as f64 / scale);
                    coords.push(node.lat() as f64 / scale);
                }
            }
            // A line string needs at least two vertices.
            if coords.len() >= 4 {
                features.push(OwnedFeature {
                    id: 0,
                    geom_type: OsmflatGeomType::LineString,
                    coords,
                    tags: collect_tags(archive, way.tags()),
                });
            }
        }
    }

    // Assign stable 1-based ids in emission order.
    for (i, f) in features.iter_mut().enumerate() {
        f.id = (i + 1) as u64;
    }

    Box::into_raw(Box::new(OsmflatFeatureSet { features, pos: 0 }))
}

/// Frees a feature set from `osmflat_query`.
///
/// # Safety
/// `fs` must be a pointer from `osmflat_query` (or null) and unused afterward.
#[no_mangle]
pub unsafe extern "C" fn osmflat_featureset_free(fs: *mut OsmflatFeatureSet) {
    if !fs.is_null() {
        drop(Box::from_raw(fs));
    }
}

/// Advances to the next feature. Returns true if a feature is now current
/// (readable via the getters below), false when iteration is exhausted.
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

/// Id of the current feature (0 if none current).
///
/// # Safety
/// `fs` must be a valid handle from `osmflat_query`.
#[no_mangle]
pub unsafe extern "C" fn osmflat_feature_id(fs: *const OsmflatFeatureSet) -> u64 {
    fs.as_ref()
        .and_then(|fs| fs.current())
        .map(|f| f.id)
        .unwrap_or(0)
}

/// Geometry type of the current feature.
///
/// # Safety
/// `fs` must be a valid handle from `osmflat_query`.
#[no_mangle]
pub unsafe extern "C" fn osmflat_feature_geom_type(fs: *const OsmflatFeatureSet) -> OsmflatGeomType {
    match fs.as_ref().and_then(|fs| fs.current()) {
        Some(f) => match f.geom_type {
            OsmflatGeomType::Point => OsmflatGeomType::Point,
            OsmflatGeomType::LineString => OsmflatGeomType::LineString,
        },
        None => OsmflatGeomType::Point,
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

/// Pointer to the current feature's interleaved `[x0, y0, x1, y1, ...]` coords
/// (length `2 * osmflat_feature_num_coords`). Valid until the next
/// `osmflat_featureset_next` or `osmflat_featureset_free`.
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

/// Number of tags on the current feature.
///
/// # Safety
/// `fs` must be a valid handle from `osmflat_query`.
#[no_mangle]
pub unsafe extern "C" fn osmflat_feature_num_tags(fs: *const OsmflatFeatureSet) -> usize {
    fs.as_ref()
        .and_then(|fs| fs.current())
        .map(|f| f.tags.len())
        .unwrap_or(0)
}

/// Key of tag `i` on the current feature. Empty (`len == 0`) if out of range.
/// Bytes are valid until the next `osmflat_featureset_next` or free.
///
/// # Safety
/// `fs` must be a valid handle from `osmflat_query`.
#[no_mangle]
pub unsafe extern "C" fn osmflat_feature_tag_key(
    fs: *const OsmflatFeatureSet,
    i: usize,
) -> OsmflatBytes {
    tag_bytes(fs, i, |t| &t.0)
}

/// Value of tag `i` on the current feature. Empty (`len == 0`) if out of range.
/// Bytes are valid until the next `osmflat_featureset_next` or free.
///
/// # Safety
/// `fs` must be a valid handle from `osmflat_query`.
#[no_mangle]
pub unsafe extern "C" fn osmflat_feature_tag_value(
    fs: *const OsmflatFeatureSet,
    i: usize,
) -> OsmflatBytes {
    tag_bytes(fs, i, |t| &t.1)
}

unsafe fn tag_bytes(
    fs: *const OsmflatFeatureSet,
    i: usize,
    select: impl Fn(&(Vec<u8>, Vec<u8>)) -> &Vec<u8>,
) -> OsmflatBytes {
    match fs.as_ref().and_then(|fs| fs.current()).and_then(|f| f.tags.get(i)) {
        Some(tag) => {
            let bytes = select(tag);
            OsmflatBytes {
                ptr: bytes.as_ptr(),
                len: bytes.len(),
            }
        }
        None => OsmflatBytes {
            ptr: std::ptr::null(),
            len: 0,
        },
    }
}
