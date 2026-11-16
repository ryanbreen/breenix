//! Recovery's choice of M (format sections 2.5 and 5, invariant S3).
//!
//! Each candidate generation, highest first, passes when its space root matches its pointer
//! and so does each FS tree root that a ROOT item written in that generation names. The
//! search for ROOT items follows the space-tree paths that can hold the ROOT key range,
//! and each node on those paths must match the entry that names it.

use std::io;

use super::image::Image;
use super::node::{self, Expect, Node};
use super::raw::{BlockPtr, Key, TREE_FS, TREE_SPACE};
use super::report::Report;
use super::slots::Slot;

pub const ROOT_OBJECT: u64 = 0xFFFF_FFFF_FFFF_FF10;
pub const ROOT_TYPE: u8 = 0x90;

/// Returns the slot holding M. When no candidate passes, reports S3 and returns no slot.
pub fn choose<'s>(image: &Image, slots: &'s [Slot], report: &mut Report) -> io::Result<Option<&'s Slot>> {
    let mut generations: Vec<u64> = slots.iter().map(|s| s.generation).collect();
    generations.sort_unstable_by(|a, b| b.cmp(a));
    generations.dedup();
    let mut rejected: Vec<(u64, String)> = Vec::new();
    for generation in generations {
        let copies = slots.iter().filter(|s| s.generation == generation);
        let slot = match copies.min_by_key(|s| s.ring) {
            Some(slot) => slot,
            None => continue,
        };
        match candidate_problem(image, slot)? {
            None => {
                for above in slots.iter().filter(|s| s.generation > generation) {
                    let why = rejected.iter().find(|(g, _)| *g == above.generation).map(|(_, r)| r.as_str());
                    report.warning(format!(
                        "{} generation {} is abandoned above M = {generation}: {}",
                        above.name(),
                        above.generation,
                        why.unwrap_or("not chosen")
                    ));
                }
                return Ok(Some(slot));
            }
            Some(reason) => rejected.push((generation, reason)),
        }
    }
    let reasons: Vec<String> = rejected.iter().map(|(g, r)| format!("generation {g}: {r}")).collect();
    report.error("S3", format!("no valid slot's roots match their pointers ({})", reasons.join("; ")));
    Ok(None)
}

fn candidate_problem(image: &Image, slot: &Slot) -> io::Result<Option<String>> {
    let ptr = slot.space_root;
    let expect = Expect {
        addr: ptr.addr,
        tree: TREE_SPACE,
        generation: Some(ptr.generation),
        csum: Some(ptr.csum),
        level: Some((ptr.level, "N1")),
        uuid: &slot.uuid,
    };
    let mut roots = Vec::new();
    if let Err(reason) = find_roots(image, slot, &expect, &mut roots)? {
        return Ok(Some(reason));
    }
    for (id, root) in roots.iter().filter(|(_, r)| r.generation == slot.generation) {
        let expect = Expect {
            addr: root.addr,
            tree: TREE_FS,
            generation: Some(root.generation),
            csum: Some(root.csum),
            level: Some((root.level, "N1")),
            uuid: &slot.uuid,
        };
        if let Err(reason) = verify(image, slot, &expect)? {
            return Ok(Some(format!("root {id}: {reason}")));
        }
    }
    Ok(None)
}

/// Reads the node `expect` names, or the reason it fails its pointer.
fn verify(image: &Image, slot: &Slot, expect: &Expect<'_>) -> io::Result<Result<Node, String>> {
    if !node::in_node_area(expect.addr, slot.total_blocks) {
        return Ok(Err(format!("node {} outside the node area", expect.addr)));
    }
    let n = node::read(image, expect.addr)?;
    Ok(match node::compare(&n, expect).first() {
        Some(m) => Err(format!("node {}: {}", expect.addr, m.detail)),
        None => Ok(n),
    })
}

fn find_roots(
    image: &Image,
    slot: &Slot,
    expect: &Expect<'_>,
    out: &mut Vec<(u64, BlockPtr)>,
) -> io::Result<Result<(), String>> {
    let n = match verify(image, slot, expect)? {
        Ok(n) => n,
        Err(reason) => return Ok(Err(reason)),
    };
    let range_lo = Key { object: ROOT_OBJECT, ty: ROOT_TYPE, offset: 0 };
    let range_hi = Key { object: ROOT_OBJECT, ty: ROOT_TYPE, offset: u64::MAX };
    if n.level == 0 {
        for item in node::leaf_items(&n) {
            if item.key.object != ROOT_OBJECT || item.key.ty != ROOT_TYPE {
                continue;
            }
            if let Some(payload) = item.payload(&n).filter(|p| p.len() >= 24) {
                out.push((item.key.offset, BlockPtr::parse(payload, 0)));
            }
        }
        return Ok(Ok(()));
    }
    let entries = node::entries(&n);
    for (i, entry) in entries.iter().enumerate() {
        let next = entries.get(i + 1).map(|e| e.key);
        let may_hold = entry.key <= range_hi && next.is_none_or(|k| k > range_lo);
        if !may_hold {
            continue;
        }
        let child = Expect {
            addr: entry.child,
            tree: TREE_SPACE,
            generation: Some(entry.generation),
            csum: Some(entry.csum),
            level: Some((n.level - 1, "N3")),
            uuid: &slot.uuid,
        };
        if let Err(reason) = find_roots(image, slot, &child, out)? {
            return Ok(Err(reason));
        }
    }
    Ok(Ok(()))
}
