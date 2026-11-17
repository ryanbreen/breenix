//! Physical ownership reporting through the core's verified tree walks.
use super::*;
use breenfs::btree::{self, NodeStore};
use format::{BlockPtr, Extent, LeafItemHeader, NodeHeader};
use std::cell::RefCell;

// Retain the encoded nodes the shared walker verified, to locate leaf items
// without adding a second B-tree implementation to the extractor.
struct Walk<'a> {
    store: DeviceStore<'a>,
    nodes: RefCell<BTreeMap<u64, Vec<u8>>>,
}
impl NodeStore for Walk<'_> {
    fn get(&self, addr: u64) -> std::result::Result<Vec<u8>, btree::Error> {
        let bytes = self.store.get(addr)?;
        self.nodes.borrow_mut().insert(addr, bytes.clone());
        Ok(bytes)
    }
    fn put(&mut self, _: u64, _: Vec<u8>) -> std::result::Result<(), btree::Error> {
        Err(btree::Error::Store)
    }
    fn alloc(&mut self) -> std::result::Result<u64, btree::Error> {
        Err(btree::Error::Store)
    }
}
struct Tree {
    items: Vec<(format::Key, Vec<u8>)>,
    nodes: BTreeMap<u64, Vec<u8>>,
    missing: Option<(u8, format::Key, Option<format::Key>)>,
}
impl Tree {
    fn read(device: &Image, root: BlockPtr) -> Result<Self> {
        let walk = Walk {
            store: DeviceStore(device),
            nodes: RefCell::new(BTreeMap::new()),
        };
        let items = core(ops::items(&walk, root))?;
        Ok(Self {
            items,
            nodes: walk.nodes.into_inner(),
            missing: None,
        })
    }
    // A forensic walk uses verified parents instead of bytes from the corrupt
    // allocation. Its parent key interval identifies candidate owners only.
    fn read_for_owner(device: &Image, root: BlockPtr, target: u64) -> Result<Self> {
        let mut tree = Self {
            items: Vec::new(),
            nodes: BTreeMap::new(),
            missing: None,
        };
        tree.visit(device, root, target, format::Key::default(), None)?;
        Ok(tree)
    }
    fn visit(
        &mut self,
        device: &Image,
        ptr: BlockPtr,
        target: u64,
        lower: format::Key,
        upper: Option<format::Key>,
    ) -> Result<()> {
        let store = DeviceStore(device);
        if let Err(error) = store.read_node(ptr) {
            if ptr.addr != target {
                return Err(format!("core error: {error:?}").into());
            }
            self.missing = Some((ptr.level, lower, upper));
            return Ok(());
        }
        let bytes = core(store.get(ptr.addr))?;
        let h = core(NodeHeader::decode(&bytes))?;
        if h.level == 0 {
            for i in 0..usize::from(h.nitems) {
                let item = core(LeafItemHeader::decode(&bytes[48 + i * 21..]))?;
                let at = usize::from(item.data_off);
                self.items.push((
                    item.key,
                    bytes[at..at + usize::from(item.data_len)].to_vec(),
                ));
            }
        } else {
            for i in 0..usize::from(h.nitems) {
                let entry = core(format::InternalEntry::decode(&bytes[48 + i * 37..]))?;
                let bound = if i + 1 < usize::from(h.nitems) {
                    Some(core(format::Key::decode(&bytes[48 + (i + 1) * 37..]))?)
                } else {
                    upper
                };
                self.visit(
                    device,
                    BlockPtr {
                        addr: entry.child_addr,
                        generation: entry.child_generation,
                        csum: entry.child_csum,
                        level: h.level - 1,
                    },
                    target,
                    entry.key,
                    bound,
                )?;
            }
        }
        self.nodes.insert(ptr.addr, bytes);
        Ok(())
    }
    fn leaf_keys(bytes: &[u8]) -> Result<Vec<format::Key>> {
        let header = core(NodeHeader::decode(bytes))?;
        (0..header.nitems as usize)
            .map(|i| Ok(core(LeafItemHeader::decode(&bytes[48 + i * 21..]))?.key))
            .collect()
    }
    fn objects(&self, addr: u64) -> Result<BTreeSet<u64>> {
        let bytes = self.nodes.get(&addr).ok_or("missing walked node")?;
        let header = core(NodeHeader::decode(bytes))?;
        let mut objects = BTreeSet::new();
        if header.level == 0 {
            for key in Self::leaf_keys(bytes)? {
                objects.insert(if key.item_type == format::ORPHAN {
                    key.offset
                } else {
                    key.object
                });
            }
        } else {
            for i in 0..header.nitems as usize {
                let entry = core(format::InternalEntry::decode(&bytes[48 + i * 37..]))?;
                objects.extend(self.objects(entry.child_addr)?);
            }
        }
        Ok(objects)
    }
    fn names(&self) -> Result<BTreeMap<u64, BTreeSet<Vec<u8>>>> {
        let mut children: BTreeMap<u64, Vec<format::DirRecord>> = BTreeMap::new();
        for (key, bytes) in &self.items {
            if key.item_type == format::DIR_INDEX {
                children
                    .entry(key.object)
                    .or_default()
                    .extend(core(ops::dir_records(bytes))?);
            }
        }
        let mut names: BTreeMap<u64, BTreeSet<Vec<u8>>> = BTreeMap::new();
        let mut dirs = BTreeSet::new();
        let mut todo = vec![(2, b"/".to_vec())];
        while let Some((ino, path)) = todo.pop() {
            names.entry(ino).or_default().insert(path.clone());
            if !dirs.insert(ino) {
                return Err("directory cycle or alias".into());
            }
            for entry in children.get(&ino).into_iter().flatten() {
                let child = join(&path, &entry.name);
                if entry.kind == 2 {
                    todo.push((entry.ino, child));
                } else {
                    names.entry(entry.ino).or_default().insert(child);
                }
            }
        }
        Ok(names)
    }
}

fn display_path(path: &[u8]) -> String {
    use std::fmt::Write;
    let mut text = String::new();
    for &byte in path {
        if byte.is_ascii_graphic() && byte != b'\\' {
            text.push(byte as char);
        } else {
            write!(text, "\\x{byte:02x}").unwrap();
        }
    }
    text
}

pub(super) fn blocks(
    device: &Image,
    sb: &Superblock,
    path: &[u8],
    out: &mut impl Write,
) -> Result<()> {
    let files = core(Files::new(DeviceStore(device), sb, 1))?;
    let ino = resolve(&files, path, true)?;
    if kind(core(files.inode(ino))?.mode)? != "reg" {
        return Err("blocks requires a regular file".into());
    }
    let bytes = core(btree::search(
        &DeviceStore(device),
        sb.space_root,
        ops::key(0xffff_ffff_ffff_ff10, format::ROOT, 1),
    ))?
    .ok_or("missing default root")?;
    let tree = Tree::read(device, core(Root::decode(&bytes))?.root)?;
    let inode_key = ops::key(ino, format::INODE, 0);
    let mut leaf = None;
    for (&addr, bytes) in &tree.nodes {
        if core(NodeHeader::decode(bytes))?.level == 0
            && Tree::leaf_keys(bytes)?.contains(&inode_key)
        {
            leaf = Some(addr);
        }
    }
    let leaf = leaf.ok_or("missing inode leaf")?;
    for (key, bytes) in &tree.items {
        if key.object == ino && key.item_type == format::EXTENT {
            let extent = core(Extent::decode(bytes))?;
            for n in 0..extent.num_blocks as u64 {
                let addr = extent
                    .phys_start
                    .checked_add(extent.extent_offset as u64)
                    .and_then(|start| start.checked_add(n))
                    .ok_or("block address overflow")?;
                let offset = key
                    .offset
                    .checked_add(n * BLOCK_SIZE as u64)
                    .ok_or("file offset overflow")?;
                writeln!(out, "data {addr} {offset}")?;
            }
        }
    }
    writeln!(out, "inode-leaf {leaf}")?;
    Ok(())
}

pub(super) fn owners(
    device: &Image,
    sb: &Superblock,
    block: u64,
    out: &mut impl Write,
) -> Result<()> {
    if block >= sb.total_blocks {
        return Err("block is outside the filesystem".into());
    }
    if (16..20).contains(&block) || block >= sb.total_blocks - 4 {
        writeln!(out, "superblock")?;
        return Ok(());
    }
    let space = core(ops::items(&DeviceStore(device), sb.space_root))?;
    let mut allocation = None;
    for (key, bytes) in &space {
        let (start, length) = match key.item_type {
            format::REF => (key.object, key.offset),
            format::DEAD => (key.object, 4),
            format::FREE => (key.offset, core(format::Free::decode(bytes))?.length),
            _ => continue,
        };
        if block >= start && block - start < length {
            if key.item_type == format::FREE {
                writeln!(out, "free")?;
                return Ok(());
            }
            allocation = Some((
                start,
                if key.item_type == format::DEAD {
                    1
                } else {
                    core(format::Ref::decode(bytes))?.kind
                },
            ));
            break;
        }
    }
    let (addr, allocation_kind) = allocation.ok_or("block is reserved or pending reuse")?;
    if allocation_kind == 2 {
        writeln!(out, "space-node")?;
        return Ok(());
    }
    let mut lines = BTreeSet::new();
    let mut level = None;
    let mut corrupt = false;
    for (_, bytes) in space
        .iter()
        .filter(|(key, _)| key.item_type == format::ROOT)
    {
        let tree = Tree::read_for_owner(device, core(Root::decode(bytes))?.root, addr)?;
        let names = tree.names()?;
        match allocation_kind {
            1 => {
                if let Some((height, lower, upper)) = tree.missing {
                    corrupt = true;
                    level = Some(height);
                    // Names come only from intact directory records. The
                    // interval may include more than the damaged leaf owns.
                    lines.insert(format!("keys={lower:?}..{upper:?}"));
                    for (&ino, paths) in &names {
                        let key = ops::key(ino, format::INODE, 0);
                        if key >= lower && upper.is_none_or(|end| key < end) {
                            lines.extend(paths.iter().map(|path| display_path(path)));
                        }
                    }
                }
                if let Some(bytes) = tree.nodes.get(&addr) {
                    level = Some(core(NodeHeader::decode(bytes))?.level);
                    for ino in tree.objects(addr)? {
                        if let Some(paths) = names.get(&ino) {
                            lines.extend(paths.iter().map(|path| display_path(path)));
                        } else {
                            lines.insert(format!("ino={ino}"));
                        }
                    }
                }
            }
            3 => {
                for (key, bytes) in &tree.items {
                    if key.item_type != format::EXTENT {
                        continue;
                    }
                    let extent = core(Extent::decode(bytes))?;
                    let start = extent
                        .phys_start
                        .checked_add(extent.extent_offset as u64)
                        .ok_or("block address overflow")?;
                    if block >= start && block - start < extent.num_blocks as u64 {
                        let offset = key
                            .offset
                            .checked_add((block - start) * BLOCK_SIZE as u64)
                            .ok_or("file offset overflow")?;
                        if let Some(paths) = names.get(&key.object) {
                            lines.extend(paths.iter().map(|path| {
                                format!("data {} offset={offset}", display_path(path))
                            }));
                        }
                    }
                }
            }
            _ => return Err("invalid allocation kind".into()),
        }
    }
    if allocation_kind == 1 {
        // DEAD nodes and their descendants need not have a namespace name.
        if level.is_none() {
            let bytes = core(DeviceStore(device).get(addr))?;
            let header = match NodeHeader::decode(&bytes) {
                Ok(header) => header,
                Err(_) => {
                    writeln!(out, "fs-node corrupt detached allocation={addr}")?;
                    return Ok(());
                }
            };
            if header.tree != 1 || header.fs_uuid != sb.fs_uuid || header.generation > sb.generation
            {
                return Err("invalid filesystem node".into());
            }
            let ptr = BlockPtr {
                addr,
                generation: header.generation,
                level: header.level,
                csum: u32::from_le_bytes(bytes[8..12].try_into()?),
            };
            let tree = Tree::read(device, ptr)?;
            lines.extend(
                tree.objects(addr)?
                    .into_iter()
                    .map(|ino| format!("ino={ino}")),
            );
            level = Some(header.level);
        }
        writeln!(
            out,
            "fs-node level={}{}{}",
            level.unwrap(),
            if corrupt { " corrupt candidates" } else { "" },
            lines
                .into_iter()
                .map(|name| format!(" {name}"))
                .collect::<String>()
        )?;
    } else {
        if lines.is_empty() {
            return Err("allocated data block has no mapped path".into());
        }
        for line in lines {
            writeln!(out, "{line}")?;
        }
    }
    Ok(())
}
