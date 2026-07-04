//! Diagnostic: list relations whose tags match ALL `key=value` terms (the
//! `member_of` filter semantics), with their full tag lists and member counts.
//! Usage: cargo run --example member_query -- <archive-dir> key=value[,key=value...]
//! A term without a value (or with `*`) matches any relation carrying the key.

use osmflat::{find_tag, relation_id, FileResourceStorage, Osm, RelationMembersRef};

fn main() {
    let usage = "usage: member_query <archive-dir> key=value[,key=value...]";
    let mut args = std::env::args().skip(1);
    let path = args.next().expect(usage);
    let spec = args.next().expect(usage);

    let archive = Osm::open(FileResourceStorage::new(&path)).expect("open archive");

    let filters: Vec<(&str, Option<&str>)> = spec
        .split(',')
        .map(|term| term.trim())
        .filter(|term| !term.is_empty())
        .map(|term| match term.split_once('=') {
            Some((k, v)) if !v.is_empty() && v != "*" => (k.trim(), Some(v.trim())),
            Some((k, _)) => (k.trim(), None),
            None => (term, None),
        })
        .collect();

    let strings = archive.stringtable();
    let tags = archive.tags();
    let tags_index = archive.tags_index();

    let mut matched = 0usize;
    for i in 0..archive.relations().len() {
        let range = archive.relations()[i].tags();
        let all = filters.iter().all(|(key, val)| match val {
            Some(v) => find_tag(&archive, range.clone(), key.as_bytes()) == Some(v.as_bytes()),
            None => find_tag(&archive, range.clone(), key.as_bytes()).is_some(),
        });
        if !all {
            continue;
        }
        matched += 1;
        println!("relation idx={} id={:?}", i, relation_id(&archive, i));
        for t in range.clone() {
            let tag = &tags[tags_index[t as usize].value() as usize];
            println!(
                "  {} = {}",
                String::from_utf8_lossy(strings.substring_raw(tag.key_idx() as usize)),
                String::from_utf8_lossy(strings.substring_raw(tag.value_idx() as usize)),
            );
        }
        let (mut n, mut w, mut r) = (0u64, 0u64, 0u64);
        for member in archive.relation_members().at(i) {
            match member {
                RelationMembersRef::NodeMember(_) => n += 1,
                RelationMembersRef::WayMember(_) => w += 1,
                RelationMembersRef::RelationMember(_) => r += 1,
            }
        }
        println!("  members: {n} nodes, {w} ways, {r} relations");
    }
    println!("{matched} matching relation(s)");
}
