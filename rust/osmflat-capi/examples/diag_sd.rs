//! Diagnostic: trace ring assembly for the South Dakota boundary relation.
//! Usage: cargo run --example diag_sd -- <archive-path> [relation-name]

use osmflat::{find_tag, relation_id, FileResourceStorage, Osm, RelationMembersRef};

const RING_SNAP_TOLERANCE_M: f64 = 250.0;
const EXACT_EPS_M: f64 = 0.01;

fn dist_m(a: (f64, f64), b: (f64, f64)) -> f64 {
    const R: f64 = 6_378_137.0;
    let (lon1, lat1) = (a.0.to_radians(), a.1.to_radians());
    let (lon2, lat2) = (b.0.to_radians(), b.1.to_radians());
    let x = (lon2 - lon1) * ((lat1 + lat2) / 2.0).cos();
    let y = lat2 - lat1;
    R * (x * x + y * y).sqrt()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("archive path");
    let name_filter = args.next().unwrap_or_else(|| "South Dakota".to_string());

    let archive = Osm::open(FileResourceStorage::new(&path)).expect("open archive");
    let scale = archive.header().coord_scale() as f64;
    let nodes = archive.nodes();
    let nodes_index = archive.nodes_index();
    let ways = archive.ways();
    let strings = archive.stringtable();
    let members = archive.relation_members();

    for (idx, relation) in archive.relations().iter().enumerate() {
        let Some(name) = find_tag(&archive, relation.tags(), b"name") else {
            continue;
        };
        if name != name_filter.as_bytes() {
            continue;
        }
        let rel_type = find_tag(&archive, relation.tags(), b"type")
            .map(|v| String::from_utf8_lossy(v).into_owned());
        let admin = find_tag(&archive, relation.tags(), b"admin_level")
            .map(|v| String::from_utf8_lossy(v).into_owned());
        println!(
            "== relation idx={} id={:?} name={} type={:?} admin_level={:?}",
            idx,
            relation_id(&archive, idx),
            name_filter,
            rel_type,
            admin
        );

        let mut n_members = 0usize;
        let mut n_way_members = 0usize;
        let mut n_unresolved_ways = 0usize;
        let mut n_short = 0usize;
        let mut outer_segs: Vec<(u64, Vec<(f64, f64)>, usize, usize)> = Vec::new();
        for member in members.at(idx) {
            n_members += 1;
            let RelationMembersRef::WayMember(wm) = member else {
                continue;
            };
            n_way_members += 1;
            let Some(way_idx) = wm.way_idx() else {
                n_unresolved_ways += 1;
                continue;
            };
            let way = &ways[way_idx as usize];
            let refs = way.refs();
            let total_refs = (refs.end - refs.start) as usize;
            let seg_idx: Vec<u64> = (refs.start as usize..refs.end as usize)
                .filter_map(|i| nodes_index[i].value())
                .collect();
            let missing_nodes = total_refs - seg_idx.len();
            if seg_idx.len() < 2 {
                n_short += 1;
                continue;
            }
            let role = strings.substring_raw(wm.role_idx() as usize);
            if role == b"inner" {
                continue;
            }
            let coords: Vec<(f64, f64)> = seg_idx
                .iter()
                .map(|&n| {
                    let node = &nodes[n as usize];
                    (node.lon() as f64 / scale, node.lat() as f64 / scale)
                })
                .collect();
            outer_segs.push((way_idx, coords, missing_nodes, total_refs));
        }
        println!(
            "members={} way_members={} unresolved_ways={} too_short={} outer_segs={}",
            n_members,
            n_way_members,
            n_unresolved_ways,
            n_short,
            outer_segs.len()
        );
        let with_missing: Vec<_> = outer_segs
            .iter()
            .filter(|(_, _, m, _)| *m > 0)
            .map(|(w, c, m, t)| (*w, c.len(), *m, *t))
            .collect();
        if !with_missing.is_empty() {
            println!("segments with missing node refs (way_idx, kept, missing, total):");
            for row in &with_missing {
                println!("  {:?}", row);
            }
        }

        // Simulate assemble_rings, reporting each failure gap.
        let mut segments: Vec<Vec<(f64, f64)>> =
            outer_segs.iter().map(|(_, c, _, _)| c.clone()).collect();
        let mut rings = 0usize;
        let mut dropped = 0usize;
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
                    rings += 1;
                    println!(
                        "ring closed: {} pts, {} stitches, final gap {:.2} m",
                        ring.len(),
                        stitched,
                        close_dist
                    );
                    break;
                }
                let end = *ring.last().unwrap();
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
                        ring.extend_from_slice(&seg[1..]);
                        stitched += 1;
                    }
                    None => {
                        dropped += 1;
                        // Report the nearest remaining endpoint to each open end.
                        let start = *ring.first().unwrap();
                        let nearest = |p: (f64, f64)| {
                            segments
                                .iter()
                                .flat_map(|s| [*s.first().unwrap(), *s.last().unwrap()])
                                .map(|q| dist_m(p, q))
                                .fold(f64::INFINITY, f64::min)
                        };
                        println!(
                            "PARTIAL RING DROPPED: {} pts, {} stitches; open ends {:.1} m apart; \
                             nearest remaining endpoint: to head {:.1} m, to tail {:.1} m; \
                             head=({:.5},{:.5}) tail=({:.5},{:.5}); {} segments left",
                            ring.len(),
                            stitched,
                            close_dist,
                            nearest(start),
                            nearest(end),
                            start.0,
                            start.1,
                            end.0,
                            end.1,
                            segments.len()
                        );
                        break;
                    }
                }
            }
        }
        println!("=> rings={} dropped_partials={}", rings, dropped);
    }
}
