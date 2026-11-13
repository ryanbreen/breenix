//! Space accounting, invariants A1-A6 (format section 3), over what the walk collected: the
//! set L (each node address once), the REF, DEAD, FREE and PENDING items of M's space tree,
//! the EXTENT items in L's leaves, and the references from `space_root`, ROOT items,
//! internal entries and EXTENT items.

use std::collections::{BTreeMap, BTreeSet};

use super::raw::{tree_name, FIRST_DATA_BLOCK, NODE_BLOCKS, TREE_SPACE};
use super::walk::{RefItem, Walk};

const KIND_FS_NODE: u8 = 1;
const KIND_SPACE_NODE: u8 = 2;
const KIND_DATA: u8 = 3;
const MAX_DATA_REF_BLOCKS: u64 = 256;

type Findings = Vec<(&'static str, String)>;

impl Walk<'_> {
    /// A1-A6 findings as (invariant, detail).
    pub fn space_findings(&self) -> Findings {
        let mut out = Vec::new();
        self.coverage(&mut out);
        self.free_apart(&mut out);
        let by_start = self.refs_by_start();
        self.ref_shapes(&by_start, &mut out);
        self.refcounts(&mut out);
        for &(start, length, released) in &self.pending {
            if released > self.m {
                out.push(("A6", format!("PENDING {start}+{length} released in generation {released}, above M = {}", self.m)));
            }
        }
        out
    }

    fn refs_by_start(&self) -> BTreeMap<u64, Vec<&RefItem>> {
        let mut by_start: BTreeMap<u64, Vec<&RefItem>> = BTreeMap::new();
        for r in &self.ref_items {
            by_start.entry(r.start).or_default().push(r);
        }
        by_start
    }

    /// A1: blocks 0-19 and ring B lie in no extent, and each block from 20 to ring B lies in
    /// exactly one REF, DEAD, FREE or PENDING extent. An extent with no blocks is reported
    /// too, since it covers nothing it claims.
    /// claim-lint:ok: docs/fs/cowfs-format.md section 3, A1.
    fn coverage(&self, out: &mut Findings) {
        let ring_b = self.total_blocks - 4;
        let mut extents: Vec<(u64, u64, String)> = Vec::new();
        for r in &self.ref_items {
            extents.push((r.start, r.length, format!("REF {}+{}", r.start, r.length)));
        }
        for &addr in &self.dead {
            extents.push((addr, NODE_BLOCKS, format!("DEAD {addr}+{NODE_BLOCKS}")));
        }
        for &(start, length) in &self.free {
            extents.push((start, length, format!("FREE {start}+{length}")));
        }
        for &(start, length, _) in &self.pending {
            extents.push((start, length, format!("PENDING {start}+{length}")));
        }
        extents.sort_unstable_by_key(|e| (e.0, e.1));

        let mut cursor = FIRST_DATA_BLOCK;
        let mut reaching = String::new();
        for (start, length, name) in extents {
            if length == 0 {
                out.push(("A1", format!("{name} covers no blocks")));
                continue;
            }
            let end = start.checked_add(length);
            if start < FIRST_DATA_BLOCK || end.is_none_or(|e| e > ring_b) {
                out.push(("A1", format!("{name} covers reserved blocks (0-19, or ring B from {ring_b})")));
            }
            let (s, e) = (start.max(FIRST_DATA_BLOCK), end.unwrap_or(u64::MAX).min(ring_b));
            if s >= e {
                continue;
            }
            if s > cursor {
                out.push(("A1", format!("blocks {cursor}-{} lie in no REF, DEAD, FREE or PENDING extent", s - 1)));
            } else if s < cursor {
                out.push(("A1", format!("blocks {s}-{} lie in both {reaching} and {name}", e.min(cursor) - 1)));
            }
            if e > cursor {
                cursor = e;
                reaching = name;
            }
        }
        if cursor < ring_b {
            out.push(("A1", format!("blocks {cursor}-{} lie in no REF, DEAD, FREE or PENDING extent", ring_b - 1)));
        }
    }

    /// A2: FREE extents are apart. Two that touch or overlap describe one free range in two
    /// items. Each extent is compared with the FREE extent before it that reaches furthest.
    /// claim-lint:ok: docs/fs/cowfs-format.md section 3, A2 (no two FREE extents are adjacent).
    fn free_apart(&self, out: &mut Findings) {
        let mut free = self.free.clone();
        free.sort_unstable();
        let mut reaching: Option<(u64, u64, u64)> = None;
        for (start, length) in free {
            let end = start.saturating_add(length);
            if let Some((ps, pl, pend)) = reaching {
                if start < pend {
                    out.push(("A2", format!("FREE extents {ps}+{pl} and {start}+{length} overlap")));
                } else if start == pend {
                    out.push(("A2", format!("FREE extents {ps}+{pl} and {start}+{length} are adjacent")));
                }
            }
            if reaching.is_none_or(|(_, _, pend)| end > pend) {
                reaching = Some((start, length, end));
            }
        }
    }

    /// A3: each non-DEAD node in L has a REF of length 4 and the node's kind; node REFs cover
    /// 4 blocks and data REFs 1-256; each EXTENT names a data REF of its start and length.
    fn ref_shapes(&self, by_start: &BTreeMap<u64, Vec<&RefItem>>, out: &mut Findings) {
        let dead: BTreeSet<u64> = self.dead.iter().copied().collect();
        for (&addr, &tree) in &self.nodes {
            if dead.contains(&addr) {
                continue;
            }
            let want = if tree == TREE_SPACE { KIND_SPACE_NODE } else { KIND_FS_NODE };
            let node = format!("{} node {addr}", tree_name(tree));
            match by_start.get(&addr).map(Vec::as_slice) {
                None | Some([]) => out.push(("A3", format!("{node} has no REF"))),
                Some(refs) => {
                    if !refs.iter().any(|r| r.kind == want && r.length == NODE_BLOCKS) {
                        let found: Vec<String> = refs.iter().map(|r| format!("kind {} length {}", r.kind, r.length)).collect();
                        out.push(("A3", format!("{node} needs a kind {want} REF of length 4, has {}", found.join(", "))));
                    }
                }
            }
        }
        for r in &self.ref_items {
            let bad_node = matches!(r.kind, KIND_FS_NODE | KIND_SPACE_NODE) && r.length != NODE_BLOCKS;
            let bad_data = r.kind == KIND_DATA && !(1..=MAX_DATA_REF_BLOCKS).contains(&r.length);
            if bad_node || bad_data {
                out.push(("A3", format!("REF {}+{} of kind {} has the wrong length", r.start, r.length, r.kind)));
            }
        }
        for e in &self.extents {
            let named = by_start.get(&e.phys_start).is_some_and(|refs| refs.iter().any(|r| r.kind == KIND_DATA && r.length == e.phys_len));
            if !named {
                out.push((
                    "A3",
                    format!("EXTENT {} names blocks {}+{}, which is no data REF", e.key, e.phys_start, e.phys_len),
                ));
            }
        }
    }

    /// A4 and A5: each REF's refcount equals the references to its start; none is 0; space
    /// nodes have refcount 1; nothing references a DEAD node.
    /// claim-lint:ok: docs/fs/cowfs-format.md section 3, A4 and A5.
    fn refcounts(&self, out: &mut Findings) {
        for r in &self.ref_items {
            let refs = self.refs.get(&r.start).copied().unwrap_or(0);
            if u64::from(r.refcount) != refs {
                out.push(("A4", format!("REF {}+{} has refcount {}, and {refs} references reach it", r.start, r.length, r.refcount)));
            }
            if r.refcount == 0 {
                out.push(("A5", format!("REF {}+{} has refcount 0", r.start, r.length)));
            } else if r.kind == KIND_SPACE_NODE && r.refcount != 1 {
                out.push(("A5", format!("space node REF {}+{} has refcount {}", r.start, r.length, r.refcount)));
            }
        }
        for addr in &self.dead {
            if let Some(&n) = self.refs.get(addr).filter(|&&n| n > 0) {
                out.push(("A5", format!("DEAD node {addr} is referenced {n} times")));
            }
        }
    }
}
