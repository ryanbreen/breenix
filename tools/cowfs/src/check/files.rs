//! Filesystem-tree invariants F1-F10 (format section 3), per root.
//!
//! Each ROOT item's tree is read back from the FS nodes the walk recorded, in entry order,
//! so a node that two roots share is read once for each. A root whose tree reaches a node
//! the walk could not trust is skipped with a warning: its N1 errors already stand, and
//! checks over part of a tree would report entries that are only unreadable.

use std::collections::{BTreeMap, BTreeSet};
use std::io;

use super::raw::{u16_at, u32_at, u64_at, Key, BLOCK};
use super::report::Report;
use super::walk::{FsNode, RootItem, Walk, ORPHAN_OBJECT};

const ROOT_INO: u64 = 2;
/// Inode numbers other than the root directory's start at 16 (format 2.3).
const FIRST_INO: u64 = 16;
const BLOCK_BYTES: u128 = BLOCK as u128;

const INODE: u8 = 0x01;
const DIR_ENTRY: u8 = 0x10;
const DIR_INDEX: u8 = 0x11;
const EXTENT: u8 = 0x20;
const INLINE: u8 = 0x21;
const XATTR: u8 = 0x30;
const ORPHAN: u8 = 0x40;

const MAX_SYMLINK: usize = 4095;
const MAX_XATTR_VALUE: usize = 4096;
const MAX_DATA_REF_BLOCKS: u64 = 256;

/// Directory-record kinds (format 2.3).
const REG: u8 = 1;
const DIR: u8 = 2;
const LNK: u8 = 7;

type Findings = Vec<(&'static str, String)>;

/// The record kind of a mode's file type: S_IFREG, S_IFDIR, S_IFCHR, S_IFBLK, S_IFIFO,
/// S_IFSOCK and S_IFLNK are kinds 1-7. Any other type is none of the seven.
/// claim-lint:ok: docs/fs/cowfs-format.md section 2.3 (the seven record kinds) and F10.
fn kind_of(mode: u32) -> Option<u8> {
    match mode & 0o170000 {
        0o100000 => Some(REG),
        0o040000 => Some(DIR),
        0o020000 => Some(3),
        0o060000 => Some(4),
        0o010000 => Some(5),
        0o140000 => Some(6),
        0o120000 => Some(LNK),
        _ => None,
    }
}

struct Inode {
    mode: u32,
    kind: Option<u8>,
    nlink: u32,
    size: u64,
    nblocks: u64,
    parent: u64,
    next_index: u64,
}

impl Inode {
    fn parse(p: &[u8]) -> Inode {
        let mode = u32_at(p, 0);
        Inode {
            mode,
            kind: kind_of(mode),
            nlink: u32_at(p, 4),
            size: u64_at(p, 16),
            nblocks: u64_at(p, 24),
            parent: u64_at(p, 32),
            next_index: u64_at(p, 48),
        }
    }
}

/// One directory record: child ino, kind, name.
struct Record<'a> {
    ino: u64,
    kind: u8,
    name: &'a [u8],
}

fn records(p: &[u8]) -> Result<Vec<Record<'_>>, String> {
    if p.is_empty() {
        return Err("holds no record".into());
    }
    let mut out = Vec::new();
    let mut at = 0;
    while at < p.len() {
        if p.len() - at < 10 || p.len() - at - 10 < p[at + 9] as usize {
            return Err(format!("record at byte {at} is cut short"));
        }
        let len = p[at + 9] as usize;
        if len == 0 {
            return Err(format!("record at byte {at} has name_len 0"));
        }
        out.push(Record { ino: u64_at(p, at), kind: p[at + 8], name: &p[at + 10..at + 10 + len] });
        at += 10 + len;
    }
    Ok(out)
}

/// One xattr record: name and value length.
struct Xattr<'a> {
    name: &'a [u8],
    value_len: usize,
}

fn xattrs(p: &[u8]) -> Result<Vec<Xattr<'_>>, String> {
    if p.is_empty() {
        return Err("holds no record".into());
    }
    let mut out = Vec::new();
    let mut at = 0;
    while at < p.len() {
        if p.len() - at < 3 {
            return Err(format!("record at byte {at} is cut short"));
        }
        let (name_len, value_len) = (p[at] as usize, u16_at(p, at + 1) as usize);
        if p.len() - at - 3 < name_len + value_len {
            return Err(format!("record at byte {at} is cut short"));
        }
        out.push(Xattr { name: &p[at + 3..at + 3 + name_len], value_len });
        at += 3 + name_len + value_len;
    }
    Ok(out)
}

/// Why a name breaks format 2.3's rule (no '/' or NUL; not `.` or `..`), if it does.
fn name_problem(name: &[u8]) -> Option<&'static str> {
    if name.contains(&b'/') {
        Some("contains '/'")
    } else if name.contains(&0) {
        Some("contains NUL")
    } else if name == b"." || name == b".." {
        Some("is . or ..")
    } else {
        None
    }
}

fn show(name: &[u8]) -> String {
    format!("{:?}", String::from_utf8_lossy(name))
}

#[derive(Default)]
struct Object<'a> {
    inode: Option<Inode>,
    entries: Vec<(u64, &'a [u8])>,
    indexes: Vec<(u64, &'a [u8])>,
    extents: Vec<(u64, &'a [u8])>,
    inlines: Vec<&'a [u8]>,
    xattrs: Vec<(u64, &'a [u8])>,
}

/// One root's items grouped by object, and the inodes its ORPHAN items name.
struct Tree<'a> {
    root: &'a RootItem,
    objects: BTreeMap<u64, Object<'a>>,
    orphans: BTreeSet<u64>,
}

impl Walk<'_> {
    /// F1-F10 for each root, as errors prefixed with the root's id; the ROOT-name half of F10
    /// once. In full mode, F7's zero tails are read from the image.
    /// claim-lint:ok: docs/fs/cowfs-format.md section 3 (F1-F10) and section 5 (full mode).
    pub fn check_files(&self, full: bool, report: &mut Report) -> io::Result<()> {
        let mut seen_names = BTreeSet::new();
        for root in &self.roots {
            if let Some(why) = name_problem(&root.name) {
                report.error("F10", format!("ROOT {} name {}: {why}", root.id, show(&root.name)));
            }
            if !seen_names.insert(root.name.as_slice()) {
                report.error("F10", format!("ROOT {} name {} repeats another root's", root.id, show(&root.name)));
            }
        }
        for root in &self.roots {
            let Some(items) = self.tree_items(root.ptr.addr) else {
                report.warning(format!("root {}: F checks skipped; its tree reaches a node that failed N1", root.id));
                continue;
            };
            let tree = Tree::build(root, &items);
            let mut out = Vec::new();
            tree.check(&self.dirhash_key, &mut out);
            if full {
                self.zero_tails(&tree, &mut out)?;
            }
            for (id, detail) in out {
                report.error(id, format!("root {}: {detail}", root.id));
            }
        }
        Ok(())
    }

    /// The items of the tree rooted at `addr`, in entry order, or None when it reaches a node
    /// the walk did not record as a trusted FS node.
    /// claim-lint:ok: Rust's Option::None; tools/cowfs/src/check/walk.rs records trusted FS nodes.
    fn tree_items(&self, addr: u64) -> Option<Vec<(Key, &[u8])>> {
        let mut out = Vec::new();
        let mut stack = vec![addr];
        let mut visited = BTreeSet::new();
        while let Some(addr) = stack.pop() {
            if !visited.insert(addr) {
                continue;
            }
            match self.fs_nodes.get(&addr)? {
                FsNode::Leaf(items) => out.extend(items.iter().map(|(k, p)| (*k, p.as_slice()))),
                FsNode::Internal(children) => stack.extend(children.iter().rev()),
            }
        }
        Some(out)
    }

    /// F7 in full mode: the bytes a regular file's extents map past its size are zero. One
    /// finding per file, at the first nonzero byte.
    /// claim-lint:ok: docs/fs/cowfs-format.md section 3, F7.
    fn zero_tails(&self, tree: &Tree<'_>, out: &mut Findings) -> io::Result<()> {
        for (&ino, obj) in &tree.objects {
            let Some(inode) = obj.inode.as_ref().filter(|i| i.kind == Some(REG)) else {
                continue;
            };
            let size = u128::from(inode.size);
            'extents: for &(offset, p) in &obj.extents {
                if p.len() != 24 {
                    continue;
                }
                let (phys_start, phys_len) = (u64_at(p, 0), u64::from(u32_at(p, 8)));
                let (skip, num) = (u64::from(u32_at(p, 12)), u64::from(u32_at(p, 16)));
                // Shapes F7 or A3 already refuse are not read.
                if phys_len > MAX_DATA_REF_BLOCKS || skip + num > phys_len {
                    continue;
                }
                for i in 0..num {
                    let file_off = u128::from(offset) + u128::from(i) * BLOCK_BYTES;
                    if file_off + BLOCK_BYTES <= size {
                        continue;
                    }
                    let block = match phys_start.checked_add(skip + i) {
                        Some(b) if b < self.image.blocks => b,
                        _ => continue,
                    };
                    let from = size.saturating_sub(file_off) as usize;
                    let data = self.image.read(block, 1)?;
                    if let Some(j) = data[from..].iter().position(|&b| b != 0) {
                        let at = file_off + (from + j) as u128;
                        out.push(("F7", format!("inode {ino}: byte {at} past size {} is not zero (block {block})", inode.size)));
                        break 'extents;
                    }
                }
            }
        }
        Ok(())
    }
}

impl<'a> Tree<'a> {
    fn build(root: &'a RootItem, items: &[(Key, &'a [u8])]) -> Tree<'a> {
        let mut objects: BTreeMap<u64, Object<'a>> = BTreeMap::new();
        let mut orphans = BTreeSet::new();
        for &(k, p) in items {
            if k.object == ORPHAN_OBJECT {
                if k.ty == ORPHAN {
                    orphans.insert(k.offset);
                }
                continue;
            }
            let obj = objects.entry(k.object).or_default();
            match k.ty {
                INODE if k.offset == 0 && p.len() == 128 => obj.inode = Some(Inode::parse(p)),
                DIR_ENTRY => obj.entries.push((k.offset, p)),
                DIR_INDEX => obj.indexes.push((k.offset, p)),
                EXTENT => obj.extents.push((k.offset, p)),
                INLINE => obj.inlines.push(p),
                XATTR => obj.xattrs.push((k.offset, p)),
                _ => {}
            }
        }
        for obj in objects.values_mut() {
            obj.extents.sort_unstable_by_key(|e| e.0);
        }
        Tree { root, objects, orphans }
    }

    fn inode(&self, ino: u64) -> Option<&Inode> {
        self.objects.get(&ino).and_then(|o| o.inode.as_ref())
    }

    fn check(&self, key: &[u8; 16], out: &mut Findings) {
        self.inodes(out);
        let links = self.directories(key, out);
        self.link_counts(&links, out);
        self.parent_chains(out);
        for (&ino, obj) in &self.objects {
            if let Some(inode) = &obj.inode {
                self.contents(ino, inode, obj, &links, out);
            }
            self.xattr_items(ino, obj, key, out);
        }
    }

    /// F1, F2 and the inode half of F10. An INODE's key, offset 0, is the lowest an object
    /// can hold, so F2 asks that each object hold one.
    fn inodes(&self, out: &mut Findings) {
        match self.inode(ROOT_INO) {
            None => out.push(("F1", "no inode 2".into())),
            Some(root) => {
                if root.kind != Some(DIR) {
                    out.push(("F1", format!("inode 2 has mode {:#o}, not a directory", root.mode)));
                }
                if root.parent != ROOT_INO {
                    out.push(("F1", format!("inode 2's parent is {}, not itself", root.parent)));
                }
            }
        }
        for (&ino, obj) in &self.objects {
            let Some(inode) = &obj.inode else {
                out.push(("F2", format!("object {ino} has items but no INODE item")));
                continue;
            };
            if ino >= self.root.next_inode {
                out.push(("F10", format!("inode {ino} is not below next_inode {}", self.root.next_inode)));
            }
            if ino != ROOT_INO && ino < FIRST_INO {
                out.push(("F10", format!("inode {ino}: inode numbers other than 2 start at 16")));
            }
            if inode.kind.is_none() {
                out.push(("F10", format!("inode {ino} mode {:#o} holds none of the seven file types", inode.mode)));
            }
        }
    }

    /// F3 and F4 for every object holding DIR_ENTRY or DIR_INDEX items. Returns who names
    /// what, for F5 and F8.
    /// claim-lint:ok: docs/fs/cowfs-format.md section 3, F3 and F4.
    fn directories(&self, key: &[u8; 16], out: &mut Findings) -> Links {
        let mut links = Links::default();
        for (&dir, obj) in &self.objects {
            if obj.entries.is_empty() && obj.indexes.is_empty() {
                continue;
            }
            let not_dir = obj.inode.as_ref().is_some_and(|i| i.kind != Some(DIR));
            let mut by_entry: BTreeMap<(&[u8], u64, u8), usize> = BTreeMap::new();
            for &(offset, p) in &obj.entries {
                if not_dir {
                    out.push(("F3", format!("inode {dir} holds DIR_ENTRY {offset:#x} but is not a directory")));
                }
                let recs = match records(p) {
                    Ok(recs) => recs,
                    Err(why) => {
                        out.push(("F3", format!("directory {dir} DIR_ENTRY {offset:#x}: {why}")));
                        continue;
                    }
                };
                let mut names = BTreeSet::new();
                for r in recs {
                    let label = format!("directory {dir} entry {} (inode {})", show(r.name), r.ino);
                    if let Some(why) = name_problem(r.name) {
                        out.push(("F3", format!("{label}: name {why}")));
                    }
                    if !names.insert(r.name) {
                        out.push(("F3", format!("{label}: name repeated within DIR_ENTRY {offset:#x}")));
                    }
                    let hash = cowfs::siphash::hash(key, r.name);
                    if hash != offset {
                        out.push(("F3", format!("{label}: at offset {offset:#x}, but SipHash of its name is {hash:#x}")));
                    }
                    let child = self.inode(r.ino);
                    match child {
                        None => out.push(("F3", format!("{label}: inode {} has no INODE item", r.ino))),
                        Some(c) if c.kind != Some(r.kind) => {
                            out.push(("F3", format!("{label}: record kind {}, inode mode {:#o}", r.kind, c.mode)));
                        }
                        Some(_) => {}
                    }
                    *by_entry.entry((r.name, r.ino, r.kind)).or_default() += 1;
                    links.named_by.entry(r.ino).or_default().push(dir);
                    *links.entries.entry(dir).or_default() += 1;
                    if child.is_some_and(|c| c.kind == Some(DIR)) {
                        *links.subdirs.entry(dir).or_default() += 1;
                    }
                }
            }
            let mut by_index: BTreeMap<(&[u8], u64, u8), Vec<u64>> = BTreeMap::new();
            for &(offset, p) in &obj.indexes {
                if not_dir {
                    out.push(("F4", format!("inode {dir} holds DIR_INDEX {offset} but is not a directory")));
                }
                if let Some(inode) = obj.inode.as_ref().filter(|i| i.kind == Some(DIR)) {
                    if offset < 2 || offset >= inode.next_index {
                        out.push(("F4", format!("directory {dir} DIR_INDEX {offset} outside [2, {})", inode.next_index)));
                    }
                }
                match records(p) {
                    Ok(recs) if recs.len() == 1 => {
                        let r = &recs[0];
                        by_index.entry((r.name, r.ino, r.kind)).or_default().push(offset);
                    }
                    Ok(recs) => out.push(("F4", format!("directory {dir} DIR_INDEX {offset} holds {} records, not one", recs.len()))),
                    Err(why) => out.push(("F4", format!("directory {dir} DIR_INDEX {offset}: {why}"))),
                }
            }
            let names: BTreeSet<(&[u8], u64, u8)> = by_entry.keys().chain(by_index.keys()).copied().collect();
            for name in names {
                let entries = by_entry.get(&name).copied().unwrap_or(0);
                let indexes = by_index.get(&name).map_or(&[][..], Vec::as_slice);
                if entries != indexes.len() {
                    let (n, ino, kind) = name;
                    out.push((
                        "F4",
                        format!(
                            "directory {dir} entry {} (inode {ino}, kind {kind}): {entries} DIR_ENTRY records, DIR_INDEX at {indexes:?}",
                            show(n)
                        ),
                    ));
                }
            }
        }
        links
    }

    /// F5: link counts, and each directory but inode 2 named once, in its parent.
    fn link_counts(&self, links: &Links, out: &mut Findings) {
        for (&ino, obj) in &self.objects {
            let Some(inode) = &obj.inode else { continue };
            let named_by = links.named_by.get(&ino).map_or(&[][..], Vec::as_slice);
            match inode.kind {
                Some(DIR) => {
                    let subdirs = links.subdirs.get(&ino).copied().unwrap_or(0);
                    if u64::from(inode.nlink) != 2 + subdirs {
                        out.push(("F5", format!("directory {ino} nlink {}, but it holds {subdirs} subdirectories", inode.nlink)));
                    }
                    if ino != ROOT_INO {
                        match named_by {
                            [by] if *by == inode.parent => {}
                            [by] => out.push(("F5", format!("directory {ino} is named in {by}, but its parent is {}", inode.parent))),
                            _ => out.push(("F5", format!("directory {ino} is named by {} entries, not one", named_by.len()))),
                        }
                    }
                }
                Some(_) => {
                    if inode.nlink as usize != named_by.len() {
                        out.push(("F5", format!("inode {ino} nlink {}, but {} entries name it", inode.nlink, named_by.len())));
                    }
                    if inode.nlink == 0 && !self.orphans.contains(&ino) {
                        out.push(("F5", format!("inode {ino} has nlink 0 and no ORPHAN item")));
                    }
                }
                None => {}
            }
        }
    }

    /// F6: every directory's parent chain reaches inode 2.
    /// claim-lint:ok: docs/fs/cowfs-format.md section 3, F6.
    fn parent_chains(&self, out: &mut Findings) {
        let dirs: BTreeMap<u64, u64> = self
            .objects
            .iter()
            .filter_map(|(&ino, o)| o.inode.as_ref().filter(|i| i.kind == Some(DIR)).map(|i| (ino, i.parent)))
            .collect();
        for (&dir, &first) in &dirs {
            if dir == ROOT_INO {
                continue;
            }
            let mut parent = first;
            let mut steps = 0;
            while parent != ROOT_INO {
                let Some(&next) = dirs.get(&parent) else {
                    out.push(("F6", format!("directory {dir}'s parent chain reaches {parent}, which is not a directory")));
                    break;
                };
                steps += 1;
                if steps > dirs.len() {
                    out.push(("F6", format!("directory {dir}'s parent chain loops without reaching inode 2")));
                    break;
                }
                parent = next;
            }
        }
    }

    /// F7 (without the full-mode zero tails) and F8: what each file type may hold.
    /// claim-lint:ok: docs/fs/cowfs-format.md section 3, F7 and F8.
    fn contents(&self, ino: u64, inode: &Inode, obj: &Object<'_>, links: &Links, out: &mut Findings) {
        let has = |what: &str, n: usize, out: &mut Findings| {
            if n > 0 {
                out.push(("F8", format!("inode {ino} (mode {:#o}) has {n} {what} item(s)", inode.mode)));
            }
        };
        match inode.kind {
            Some(REG) => {
                has("INLINE", obj.inlines.len(), out);
                self.extent_geometry(ino, inode, obj, out);
            }
            Some(LNK) => {
                has("EXTENT", obj.extents.len(), out);
                match obj.inlines.as_slice() {
                    [target] if target.len() as u64 == inode.size && (1..=MAX_SYMLINK).contains(&target.len()) => {}
                    [target] => out.push((
                        "F8",
                        format!("symlink {ino} target of {} bytes, size {} (targets are 1-4095 bytes)", target.len(), inode.size),
                    )),
                    other => out.push(("F8", format!("symlink {ino} has {} INLINE items, not one", other.len()))),
                }
            }
            Some(kind) => {
                has("INLINE", obj.inlines.len(), out);
                has("EXTENT", obj.extents.len(), out);
                let want = if kind == DIR { links.entries.get(&ino).copied().unwrap_or(0) } else { 0 };
                if inode.size != want {
                    out.push(("F8", format!("inode {ino} (mode {:#o}) size {}, expected {want}", inode.mode, inode.size)));
                }
            }
            None => {}
        }
    }

    /// F7: a regular file's extents are aligned, disjoint, start below its size rounded up to
    /// 4 KiB, fit their REF and have flags 0; nblocks is their sum.
    fn extent_geometry(&self, ino: u64, inode: &Inode, obj: &Object<'_>, out: &mut Findings) {
        let limit = u128::from(inode.size).div_ceil(BLOCK_BYTES) * BLOCK_BYTES;
        let mut reached: u128 = 0;
        let mut sum: u64 = 0;
        for &(offset, p) in &obj.extents {
            if p.len() != 24 {
                continue;
            }
            let (phys_len, skip, num, flags) = (u32_at(p, 8), u32_at(p, 12), u32_at(p, 16), u32_at(p, 20));
            let label = format!("inode {ino} EXTENT at byte {offset}");
            if offset % BLOCK as u64 != 0 {
                out.push(("F7", format!("{label}: not 4 KiB aligned")));
            }
            if num == 0 {
                out.push(("F7", format!("{label}: num_blocks 0")));
            }
            if u64::from(skip) + u64::from(num) > u64::from(phys_len) {
                out.push(("F7", format!("{label}: extent_offset {skip} + num_blocks {num} exceeds phys_len {phys_len}")));
            }
            if flags != 0 {
                out.push(("F7", format!("{label}: flags {flags:#x}")));
            }
            if u128::from(offset) >= limit {
                out.push(("F7", format!("{label}: starts at or past size {} rounded up to 4 KiB", inode.size)));
            }
            if u128::from(offset) < reached {
                out.push(("F7", format!("{label}: overlaps the extent before it")));
            }
            reached = reached.max(u128::from(offset) + u128::from(num) * BLOCK_BYTES);
            sum = sum.saturating_add(u64::from(num));
        }
        if inode.nblocks != sum {
            out.push(("F7", format!("inode {ino} nblocks {}, but its extents map {sum}", inode.nblocks)));
        }
    }

    /// F9: xattr records are well formed, within limits, at offset SipHash(name) and named
    /// once per inode.
    fn xattr_items(&self, ino: u64, obj: &Object<'_>, key: &[u8; 16], out: &mut Findings) {
        let mut names = BTreeSet::new();
        for &(offset, p) in &obj.xattrs {
            let recs = match xattrs(p) {
                Ok(recs) => recs,
                Err(why) => {
                    out.push(("F9", format!("inode {ino} XATTR {offset:#x}: {why}")));
                    continue;
                }
            };
            for x in recs {
                let label = format!("inode {ino} xattr {}", show(x.name));
                if x.name.is_empty() {
                    out.push(("F9", format!("{label}: name_len 0")));
                }
                if x.value_len > MAX_XATTR_VALUE {
                    out.push(("F9", format!("{label}: value of {} bytes exceeds 4096", x.value_len)));
                }
                let hash = cowfs::siphash::hash(key, x.name);
                if hash != offset {
                    out.push(("F9", format!("{label}: at offset {offset:#x}, but SipHash of its name is {hash:#x}")));
                }
                if !names.insert(x.name) {
                    out.push(("F9", format!("{label}: name repeated")));
                }
            }
        }
    }
}

/// Who names what in one root, from its DIR_ENTRY records.
#[derive(Default)]
struct Links {
    /// Each named inode, with the directory of each record naming it.
    named_by: BTreeMap<u64, Vec<u64>>,
    /// Records per directory: a directory's size.
    entries: BTreeMap<u64, u64>,
    /// Records per directory that name a directory.
    subdirs: BTreeMap<u64, u64>,
}
