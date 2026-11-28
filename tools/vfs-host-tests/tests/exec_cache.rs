//! E3-3 (docs/plans/e3-page-cache.md 10.1-10.3): exec maps an image through
//! its file cache. The loader's plan and placement (`Headers::load_image`),
//! the exec admission, the exec bindings and the entries exec installs for
//! a cached PT_LOAD (`file_map::ExecImage::map`, `entry_flags`), the
//! running-image refusals (`MapState::open_writer`, `refuse_text_busy`) and
//! binding lifetime are production code over the production BreenFS. The
//! page table records those entries; the `bind` record, the private pages of
//! the copying loader and a store's outcome under an entry's flags are the
//! host model.
use crate::{
    block::cache::BufferCache,
    breenfs::{commit::{Committer, DeviceStore}, format::{Superblock, Time}, ops::Files, vfs::BreenFs},
    errno::Errno,
    exec_elf::{prot_bits, Headers, ImageTarget},
    memory::file_map::{
        bind_shared_authority, ExecImage, PageTableFlags, PhysFrame, ProcessPageTable, Protection, Vma,
        COW_FLAG, TEXT_BUSY,
    },
    permissions::Credentials,
    read_conformance::{Disk, NoWait},
    syscall_errno::ETXTBSY,
    vfs::*,
};
use std::{
    cell::Cell,
    collections::BTreeMap,
    sync::{atomic::{AtomicUsize, Ordering}, Arc, Mutex},
};

const PAGE: u64 = 4096;
const TEXT: u64 = 0x40_0000;
const DATA: u64 = 0x40_3000;
const DATA_FILE: u64 = 0x2000;
const DATA_BYTES: u64 = 0x1800;
const BSS: u64 = 1 << 20;
const USER_END: u64 = 0x7fff_0000_0000;
const EM_X86_64: u16 = 62;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

fn put(bytes: &mut [u8], at: usize, value: &[u8]) {
    bytes[at..at + value.len()].copy_from_slice(value);
}

/// An ET_EXEC image whose PT_LOADs are `(offset, address, filesz, memsz,
/// flags)`, with the file bytes after the headers patterned.
fn image(loads: &[(u64, u64, u64, u64, u32)], length: usize) -> Vec<u8> {
    let mut bytes: Vec<u8> = (0..length).map(|i| (i % 251) as u8 + 1).collect();
    bytes[..64 + 56 * loads.len()].fill(0);
    put(&mut bytes, 0, b"\x7fELF\x02\x01\x01");
    put(&mut bytes, 16, &2u16.to_le_bytes());
    put(&mut bytes, 18, &EM_X86_64.to_le_bytes());
    put(&mut bytes, 20, &1u32.to_le_bytes());
    put(&mut bytes, 24, &(TEXT + 0x100).to_le_bytes());
    put(&mut bytes, 32, &64u64.to_le_bytes());
    put(&mut bytes, 52, &64u16.to_le_bytes());
    put(&mut bytes, 54, &56u16.to_le_bytes());
    put(&mut bytes, 56, &(loads.len() as u16).to_le_bytes());
    for (slot, &(offset, address, filesz, memsz, flags)) in loads.iter().enumerate() {
        let at = 64 + 56 * slot;
        put(&mut bytes, at, &1u32.to_le_bytes());
        put(&mut bytes, at + 4, &flags.to_le_bytes());
        put(&mut bytes, at + 8, &offset.to_le_bytes());
        put(&mut bytes, at + 16, &address.to_le_bytes());
        put(&mut bytes, at + 32, &filesz.to_le_bytes());
        put(&mut bytes, at + 40, &memsz.to_le_bytes());
        put(&mut bytes, at + 48, &PAGE.to_le_bytes());
    }
    bytes
}

/// Text and rodata in two whole pages, then data whose last file page is
/// partial, followed by 1 MiB of BSS.
fn program() -> Vec<u8> {
    image(&[
        (0, TEXT, 2 * PAGE, 2 * PAGE, PF_R | PF_X),
        (DATA_FILE, DATA, DATA_BYTES, DATA_BYTES + BSS, PF_R | PF_W),
    ], (DATA_FILE + DATA_BYTES) as usize)
}

fn fixture(bytes: &[u8]) -> (Arc<BreenFs<NoWait>>, Arc<VNode>) {
    fixture_ranges(bytes, &[0..bytes.len()])
}

/// An image file holding only `ranges` of `bytes`; the rest is a hole.
fn fixture_ranges(bytes: &[u8], ranges: &[std::ops::Range<usize>]) -> (Arc<BreenFs<NoWait>>, Arc<VNode>) {
    let disk = Arc::new(Disk { bytes: Mutex::new(vec![0; 64 * 1024 * 1024]), writable: true });
    let sb = Superblock { fs_uuid: [0x45; 16], generation: 1, slot: 1,
        total_blocks: 16384, global_reserve: 4096, next_root_id: 2,
        ..Superblock::default() };
    let mut files = Files::format(DeviceStore(disk.as_ref()), &sb).unwrap();
    let ino = files.create(2, b"image", 0o755, Time::default()).unwrap();
    for range in ranges {
        files.write(disk.as_ref(), ino, range.start as u64, &bytes[range.clone()], Time::default()).unwrap();
    }
    files.fsync(disk.as_ref(), &mut Committer::format(sb)).unwrap();
    drop(files);
    let fs = BreenFs::mount_writable(disk, Arc::new(BufferCache::new(NoWait)), 73, 1).unwrap();
    let node = fs.open(ino).unwrap();
    (fs, node)
}

/// The image's transfer, counting the file bytes the cache reads through it.
struct Counted {
    inner: Box<dyn MappingIo>,
    bytes: Arc<AtomicUsize>,
}

impl MappingIo for Counted {
    fn as_any(&self) -> &dyn core::any::Any { self }
    fn size(&self) -> u64 { self.inner.size() }
    fn read(&self, offset: u64, length: usize) -> Result<Vec<u8>, Errno> {
        let read = self.inner.read(offset, length)?;
        self.bytes.fetch_add(read.len(), Ordering::Relaxed);
        Ok(read)
    }
    fn read_into(&self, offset: u64, bytes: &mut [u8]) -> Result<usize, Errno> {
        let read = self.inner.read_into(offset, bytes)?;
        self.bytes.fetch_add(read, Ordering::Relaxed);
        Ok(read)
    }
    fn needs_flush(&self) -> bool { self.inner.needs_flush() }
    fn sync(&self, first: u64, last: u64) -> Result<(), Errno> { self.inner.sync(first, last) }
    fn publish(&self, bind: &mut dyn FnMut(bool) -> Result<(), Errno>) -> Result<(), Errno> {
        self.inner.publish(bind)
    }
}

/// What exec installed at one virtual page.
#[derive(Clone, Copy, Debug)]
struct Entry {
    frame: usize,
    flags: u32,
    /// A cache frame, not a page this image owns.
    cached: bool,
    /// The entry production installed for a cache frame (`ExecImage::map`);
    /// a private page is mapped with its ELF flags.
    pte: Option<PageTableFlags>,
}

/// What a user store does under an entry.
#[derive(Debug, PartialEq, Eq)]
enum Store {
    /// The entry is writable: the byte lands in its frame.
    Written,
    /// A read-only copy-on-write entry of a writable VMA: the kernel's COW
    /// fault copies the frame into a private page, which takes the byte.
    Copied,
    /// No write access: SIGSEGV.
    Segv,
}

/// The new address space: entries, VMAs and the exec bindings that hold the
/// image's pages, as the kernel's `KernelImage` makes them.
struct Image<'a> {
    exec: Option<ExecImage<'a>>,
    entries: BTreeMap<u64, Entry>,
    owned: Vec<Box<[u8; 4096]>>,
    /// `(start, end, flags, file)`.
    vmas: Vec<(u64, u64, u32, bool)>,
    /// The production table and VMAs of the cached pieces; each VMA's
    /// binding holds the image's pages and its `deny_write`.
    table: ProcessPageTable,
    file_vmas: Vec<Vma>,
    fell_back: bool,
}

impl ImageTarget for Image<'_> {
    fn private_page(&mut self, address: u64, flags: u32) -> Result<*mut u8, Errno> {
        let mut page = Box::new([0u8; 4096]);
        let ptr = page.as_mut_ptr();
        let entry = Entry { frame: ptr as usize, flags, cached: false, pte: None };
        assert!(self.entries.insert(address, entry).is_none());
        self.owned.push(page);
        Ok(ptr)
    }
    fn sync(&mut self, _: *mut u8, _: usize) {}
    fn cached(&self) -> bool {
        self.exec.is_some()
    }
    fn populate(&mut self, first: u64, count: u64) -> Result<(), Errno> {
        self.exec.as_mut().unwrap().populate(first, count).map_err(|error| Errno(error.errno()))
    }
    fn map_file(&mut self, start: u64, end: u64, pgoff: u64, flags: u32) -> Result<(), Errno> {
        // The kernel target's map_file (`KernelImage`): production ExecImage::map.
        let prot = Protection::from_bits_truncate(prot_bits(flags));
        let vma = self.exec.as_ref().unwrap().map(&mut self.table, start, end, pgoff, prot).unwrap();
        for address in (start..end).step_by(PAGE as usize) {
            let (PhysFrame(frame), pte) = self.table.entries[&address];
            let entry = Entry { frame, flags, cached: true, pte: Some(pte) };
            assert!(self.entries.insert(address, entry).is_none());
        }
        self.file_vmas.push(vma);
        self.vmas.push((start, end, flags, true));
        Ok(())
    }
    fn copy_file(&mut self, index: u64, dst: *mut u8, len: usize) -> Result<(), Errno> {
        // SAFETY: `dst` is the page private_page just gave this image.
        unsafe { self.exec.as_ref().unwrap().copy(index, dst, len) }.map_err(|_| Errno(5))
    }
    fn zero(&mut self, start: u64, end: u64, flags: u32) -> Result<(), Errno> {
        self.vmas.push((start, end, flags, false));
        Ok(())
    }
    fn fall_back(&mut self) {
        self.fell_back = true;
        self.exec = None;
    }
}

impl Image<'_> {
    fn bytes(&self, address: u64) -> &[u8] {
        let entry = self.entries[&address];
        // SAFETY: a cache frame stays resident while the exec binding covers
        // it; an owned page lives in `owned`.
        unsafe { std::slice::from_raw_parts(entry.frame as *const u8, PAGE as usize) }
    }

    /// A user store of `byte` at `address`, as the CPU and the kernel's
    /// permission and copy-on-write fault paths treat the entry there.
    fn store(&mut self, address: u64, byte: u8) -> Store {
        let page = address & !(PAGE - 1);
        let entry = self.entries[&page];
        let writable = match entry.pte {
            Some(pte) => pte.contains(PageTableFlags::WRITABLE),
            None => entry.flags & PF_W != 0,
        };
        let outcome = if writable {
            Store::Written
        } else if entry.pte.is_some_and(|pte| pte.contains(COW_FLAG)) && entry.flags & PF_W != 0 {
            let mut copy = Box::new([0u8; 4096]);
            copy.copy_from_slice(self.bytes(page));
            let frame = copy.as_mut_ptr() as usize;
            self.owned.push(copy);
            self.entries.insert(page, Entry { frame, flags: entry.flags, cached: false, pte: None });
            Store::Copied
        } else {
            return Store::Segv;
        };
        let frame = self.entries[&page].frame;
        // SAFETY: the entry's frame is a cache page this image's binding holds
        // or a page in `owned`.
        unsafe { (frame as *mut u8).add((address - page) as usize).write(byte) };
        outcome
    }
}

/// Exec `node` as the kernel does (`MappedElf::load`): admit it to its cache
/// through the transfer exec holds, then read its headers and load it with
/// the production reader. `reads` counts the bytes the loader reads.
fn exec<'a>(
    fs: &dyn FileSystem,
    node: &'a Arc<VNode>,
    transfer: &'a Counted,
    reads: &Cell<usize>,
) -> Image<'a> {
    let before = transfer.bytes.load(Ordering::Relaxed);
    let (image, _) = crate::exec::read_image(
        fs, node, MountFlags::default(), &Credentials::kernel(), || false, false,
        |size, read| {
            let mut image = Image {
                exec: ExecImage::new(node, transfer),
                entries: BTreeMap::new(),
                owned: Vec::new(),
                vmas: Vec::new(),
                table: ProcessPageTable::default(),
                file_vmas: Vec::new(),
                fell_back: false,
            };
            let mut header = [0; 64];
            read(0, &mut header)?;
            let headers = Headers::read(&header, size, EM_X86_64, USER_END, read)?;
            let mut counted = |offset: u64, bytes: &mut [u8]| {
                reads.set(reads.get() + bytes.len());
                read(offset, bytes)
            };
            // SAFETY: private_page gives the loader pages the image owns.
            unsafe { headers.load_image(&mut counted, &mut image, 0)? };
            // The load ends before the table is published (`MappedElf::load`
            // drops its target): from here the exec bindings hold the image.
            image.exec = None;
            Ok(image)
        },
    ).unwrap();
    reads.set(reads.get() + transfer.bytes.load(Ordering::Relaxed) - before);
    image
}

fn transfer(node: &VNode) -> Counted {
    Counted { inner: node.filesystem.mapping_io(node).unwrap(), bytes: Arc::new(AtomicUsize::new(0)) }
}

#[test]
fn a_second_exec_shares_the_first_execs_text_and_reads_nothing() {
    let bytes = program();
    let (fs, node) = fixture(&bytes);
    let io = transfer(&node);
    let (first_reads, second_reads) = (Cell::new(0), Cell::new(0));
    let mut first = exec(fs.as_ref(), &node, &io, &first_reads);
    let second = exec(fs.as_ref(), &node, &io, &second_reads);
    assert!(first_reads.get() > 0);
    // Text, rodata and the data's whole page are the cache's frames in both.
    for address in [TEXT, TEXT + PAGE, DATA] {
        assert!(first.entries[&address].cached && second.entries[&address].cached,
            "page {address:#x} is a private copy, not the cache's frame");
        assert_eq!(first.entries[&address].frame, second.entries[&address].frame);
    }
    assert_eq!(second_reads.get(), 0, "the second exec read its segments again");
    assert_eq!(first.bytes(TEXT + PAGE), &bytes[PAGE as usize..2 * PAGE as usize]);
    // Under the entries production installed, a store to text faults and a
    // store to data copies the page; the shared cache frame keeps the file's
    // bytes for the second process running the image.
    let text_pte = first.entries[&TEXT].pte.unwrap();
    assert!(!text_pte.contains(PageTableFlags::NO_EXECUTE), "text is not executable");
    assert_eq!(first.store(TEXT + 9, 0xee), Store::Segv, "a store to running text did not fault");
    assert_eq!(first.store(DATA + 9, 0xee), Store::Copied, "a store to data wrote the shared cache frame");
    assert_eq!(first.bytes(DATA)[9], 0xee);
    let data = DATA_FILE as usize;
    assert_eq!(second.bytes(DATA), &bytes[data..data + PAGE as usize], "another process saw the data store");
    assert!(first.vmas.iter().any(|v| v.0 == DATA && v.3), "data is not a file mapping");
}

#[test]
fn bss_is_demand_zero_after_the_tail_page() {
    let bytes = program();
    let (fs, node) = fixture(&bytes);
    let io = transfer(&node);
    let image = exec(fs.as_ref(), &node, &io, &Cell::new(0));
    let tail = DATA + PAGE;
    let bss = tail + PAGE..(DATA + DATA_BYTES + BSS + PAGE - 1) & !(PAGE - 1);
    assert_eq!((bss.end - bss.start) / PAGE, 256);
    assert_eq!(image.entries.range(bss.clone()).count(), 0, "BSS pages were allocated before a touch");
    assert!(image.vmas.iter().any(|&(start, end, _, file)| !file && start <= tail && end == bss.end),
        "no demand-zero VMA covers the tail page and the BSS");
    // The tail page holds the segment's last file bytes, then 00s.
    let keep = (DATA + DATA_BYTES - tail) as usize;
    let file = (DATA_FILE + PAGE) as usize;
    assert!(!image.entries[&tail].cached);
    assert_eq!(&image.bytes(tail)[..keep], &bytes[file..file + keep]);
    assert!(image.bytes(tail)[keep..].iter().all(|&byte| byte == 0));
}

#[test]
fn a_running_image_refuses_write_access() {
    let (fs, node) = fixture(&program());
    let io = transfer(&node);
    let image = exec(fs.as_ref(), &node, &io, &Cell::new(0));
    // The exec load has ended: its exec bindings alone make this a running image.
    assert!(matches!(node.map.open_writer(), Err(Errno(ETXTBSY))));
    // mmap's refusal, made before it replaces anything, is for shared write
    // authority only.
    assert_eq!(node.map.refuse_text_busy(true, true), Err(TEXT_BUSY));
    assert_eq!(node.map.refuse_text_busy(true, false), Ok(()));
    assert_eq!(node.map.refuse_text_busy(false, true), Ok(()));
    drop(image);
    let writer = node.map.open_writer().expect("the image no longer runs");
    drop(writer);
    assert_eq!(node.map.refuse_text_busy(true, true), Ok(()));
}

#[test]
fn an_image_open_for_writing_is_copied_and_runs() {
    let bytes = program();
    let (fs, node) = fixture(&bytes);
    let io = transfer(&node);
    let writer = node.map.open_writer().unwrap();
    let image = exec(fs.as_ref(), &node, &io, &Cell::new(0));
    assert!(image.entries.values().all(|entry| !entry.cached));
    assert_eq!(image.bytes(TEXT + PAGE), &bytes[PAGE as usize..2 * PAGE as usize]);
    drop(writer);
    // The copy holds no claim: write access is not refused while it runs.
    drop(node.map.open_writer().unwrap());
}

#[test]
fn a_shared_binding_with_write_authority_cannot_change_running_text() {
    // mmap MAP_SHARED, PROT_READ from an O_RDWR descriptor; the descriptor
    // closes; exec; mprotect PROT_WRITE and store into the text page.
    let bytes = program();
    let (fs, node) = fixture(&bytes);
    let io = transfer(&node);
    let shared = bind_shared_authority(node.clone(), 2 * PAGE);
    let image = exec(fs.as_ref(), &node, &io, &Cell::new(0));
    assert!(!image.entries[&(TEXT + PAGE)].cached);
    shared.protect_write().unwrap();
    shared.store(PAGE + 7, 0xee).unwrap();
    assert_eq!(image.bytes(TEXT + PAGE), &bytes[PAGE as usize..2 * PAGE as usize]);
}

#[test]
fn a_segment_that_breaks_congruence_is_copied() {
    // The file offset is 0x100 into its page and the address is page-aligned.
    let bytes = image(&[(PAGE + 0x100, TEXT, 0x800, 0x800, PF_R | PF_X)], 2 * PAGE as usize);
    let (fs, node) = fixture(&bytes);
    let io = transfer(&node);
    let image = exec(fs.as_ref(), &node, &io, &Cell::new(0));
    assert!(image.fell_back, "a non-congruent segment did not take the counted fallback");
    assert!(!image.entries[&TEXT].cached);
    assert_eq!(&image.bytes(TEXT)[..0x800], &bytes[(PAGE + 0x100) as usize..(PAGE + 0x900) as usize]);
}

#[test]
fn exec_populates_only_the_pages_its_loads_map() {
    // One page of text at file page 0 and one of data at file page 256,
    // with a 255-page hole between them that neither load maps.
    let far = 256 * PAGE;
    let bytes = image(&[
        (0, TEXT, PAGE, PAGE, PF_R | PF_X),
        (far, TEXT + far, PAGE, PAGE, PF_R | PF_W),
    ], (far + PAGE) as usize);
    let (fs, node) = fixture_ranges(&bytes, &[0..PAGE as usize, far as usize..(far + PAGE) as usize]);
    let io = transfer(&node);
    let image = exec(fs.as_ref(), &node, &io, &Cell::new(0));
    assert!(image.entries[&TEXT].cached && image.entries[&(TEXT + far)].cached);
    assert_eq!(image.bytes(TEXT + far), &bytes[far as usize..(far + PAGE) as usize]);
    let (resident, _, _) = node.map.cache_counts();
    assert_eq!(resident, 2, "exec populated the pages between its loads, which no load maps");
    assert_eq!(io.bytes.load(Ordering::Relaxed), 2 * PAGE as usize, "exec read the hole between its loads");
}
