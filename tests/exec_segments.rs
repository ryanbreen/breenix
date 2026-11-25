//! Host checks of the production VFS exec reader with a sparse file and a
//! destination that verifies bytes as they arrive, without a 64 MiB fixture.
#![cfg(test)]
extern crate alloc;

#[path = "../kernel/src/fs/vfs/exec.rs"]
mod exec;
#[path = "../kernel/src/exec_elf.rs"]
mod exec_elf;
#[path = "../userspace/programs/src/padded_elf.rs"]
mod padded_elf;

/// Heap bytes this thread holds, and the most it has held since `begin`.
mod heap {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    thread_local! {
        static LIVE: Cell<usize> = const { Cell::new(0) };
        static PEAK: Cell<usize> = const { Cell::new(0) };
    }

    pub struct Counting;

    // SAFETY: alloc and dealloc forward their arguments unchanged to the system allocator.
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            // SAFETY: the caller's layout contract passes through.
            let ptr = unsafe { System.alloc(layout) };
            if !ptr.is_null() {
                let _ = LIVE.try_with(|live| {
                    let now = live.get() + layout.size();
                    live.set(now);
                    let _ = PEAK.try_with(|peak| peak.set(peak.get().max(now)));
                });
            }
            ptr
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            // SAFETY: the caller's pointer and layout contract passes through.
            unsafe { System.dealloc(ptr, layout) };
            let _ = LIVE.try_with(|live| live.set(live.get().saturating_sub(layout.size())));
        }
    }

    pub fn begin() -> usize {
        let live = LIVE.with(Cell::get);
        PEAK.with(|peak| peak.set(live));
        live
    }

    pub fn peak_since(start: usize) -> usize {
        PEAK.with(Cell::get).saturating_sub(start)
    }
}

#[global_allocator]
static HEAP: heap::Counting = heap::Counting;
#[path = "../kernel/src/syscall/errno.rs"]
pub mod syscall_errno;
mod syscall {
    pub use crate::syscall_errno as errno;
}
mod process {
    pub mod credentials {
        #[derive(Default)]
        pub struct ExecIdentity;
    }
}
mod fs {
    pub mod errno {
        #[derive(Debug, PartialEq, Eq)]
        pub struct Errno(pub i32);
    }
    pub mod permissions {
        use super::vfs::MountFlags;
        use crate::process::credentials::ExecIdentity;
        pub struct Credentials;
        impl Credentials {
            pub fn kernel() -> Self {
                Self
            }
            pub fn may_exec(
                &self,
                _: super::vfs::InodeAttrs,
                _: MountFlags,
            ) -> Result<ExecIdentity, i32> {
                Ok(ExecIdentity)
            }
        }
    }
    pub mod vfs {
        use super::errno::Errno;
        #[derive(Default)]
        pub struct MountFlags;
        pub struct InodeAttrs {
            pub mode: u16,
        }
        pub enum NodeKind {
            Regular,
            Directory,
            Other,
        }
        pub struct NodeAttrs {
            pub kind: NodeKind,
            pub attrs: InodeAttrs,
            pub size: u64,
        }
        pub struct Map;
        impl Map {
            pub fn may_change_exec_image(&self) -> bool {
                false
            }
        }
        pub struct VNode {
            pub ino: u64,
            pub map: Map,
        }
        impl VNode {
            pub fn file() -> Self {
                Self { ino: 1, map: Map }
            }
        }
        pub trait FileSystem {
            fn getattr(&self, ino: u64) -> Result<NodeAttrs, Errno>;
            fn read(&self, node: &VNode, offset: u64, bytes: &mut [u8]) -> Result<usize, Errno>;
        }
    }
}
use fs::{
    errno::Errno,
    permissions::Credentials,
    vfs::{FileSystem, InodeAttrs, MountFlags, NodeAttrs, NodeKind, VNode},
};
use syscall_errno::{EINTR, EIO, EISDIR};

struct SparseElf {
    header: [u8; ELF_HEADER + PHDR],
    size: usize,
    available: usize,
}

impl FileSystem for SparseElf {
    fn getattr(&self, ino: u64) -> Result<NodeAttrs, Errno> {
        let kind = match ino {
            1 => NodeKind::Regular,
            2 => NodeKind::Directory,
            _ => NodeKind::Other,
        };
        Ok(NodeAttrs {
            kind,
            attrs: InodeAttrs { mode: 0o755 },
            size: self.size as u64,
        })
    }

    fn read(&self, _: &VNode, offset: u64, bytes: &mut [u8]) -> Result<usize, Errno> {
        let start = offset as usize;
        let count = self.available.saturating_sub(start).min(bytes.len());
        bytes[..count].fill(0);
        let header_len = self.header.len().saturating_sub(start).min(count);
        if header_len > 0 {
            bytes[..header_len].copy_from_slice(&self.header[start..start + header_len]);
        }
        if count > 0 && start + count == self.size {
            bytes[count - 1] = b'Z';
        }
        Ok(count)
    }
}

const CHUNK: usize = 64 * 1024;
const ELF_HEADER: usize = 64;
const PHDR: usize = 56;
const X86_HEAP: usize = 64 * 1024 * 1024;

fn header(file_size: usize, memory_size: usize) -> [u8; ELF_HEADER + PHDR] {
    let mut bytes = [0; ELF_HEADER + PHDR];
    bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
    bytes[18..20].copy_from_slice(&62u16.to_le_bytes());
    bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
    bytes[24..32].copy_from_slice(&0x4000_0000u64.to_le_bytes());
    bytes[32..40].copy_from_slice(&(ELF_HEADER as u64).to_le_bytes());
    bytes[52..54].copy_from_slice(&(ELF_HEADER as u16).to_le_bytes());
    bytes[54..56].copy_from_slice(&(PHDR as u16).to_le_bytes());
    bytes[56..58].copy_from_slice(&1u16.to_le_bytes());
    bytes[64..68].copy_from_slice(&1u32.to_le_bytes()); // PT_LOAD
    bytes[68..72].copy_from_slice(&5u32.to_le_bytes()); // PF_R | PF_X
    bytes[80..88].copy_from_slice(&0x4000_0000u64.to_le_bytes());
    bytes[96..104].copy_from_slice(&(file_size as u64).to_le_bytes());
    bytes[104..112].copy_from_slice(&(memory_size as u64).to_le_bytes());
    bytes[112..120].copy_from_slice(&4096u64.to_le_bytes());
    bytes
}

const RING_PAGES: usize = 32;
const USER_END: u64 = 0x8000_0000_0000;

/// Exec the image through the production reader and segment loader, handing
/// each read's bytes to `copy` with its segment offset. The image's pages come
/// from a ring allocated before measuring, so only the loader's own heap use
/// counts. Returns the peak heap bytes the exec held and the loader's facts.
fn load(
    fs: &dyn FileSystem,
    node: &VNode,
    killed: impl Fn() -> bool,
    mut copy: impl FnMut(usize, &[u8]),
) -> Result<(usize, [u64; 5]), Errno> {
    let mut ring = vec![0u8; RING_PAGES * 4096];
    let base = ring.as_mut_ptr();
    let start = heap::begin();
    let (summary, _) = crate::exec::read_image(
        fs,
        node,
        MountFlags::default(),
        &Credentials::kernel(),
        killed,
        |size, read| {
            let mut header = [0; exec_elf::ELF_HEADER];
            let prefix = size.min(header.len() as u64) as usize;
            read(0, &mut header[..prefix])?;
            let headers = exec_elf::Headers::read(&header[..prefix], size, 62, USER_END, read)?;
            let mut pages = 0;
            let mut loaded = 0;
            // SAFETY: each ring page is handed out zeroed, and a read run (at
            // most CHUNK bytes, half the ring) is flushed before its pages
            // come round again.
            unsafe {
                headers.load_segments(
                    read,
                    |_, _| {
                        let page = base.add((pages % RING_PAGES) * 4096);
                        page.write_bytes(0, 4096);
                        pages += 1;
                        Ok(page)
                    },
                    |ptr, len| {
                        copy(loaded, core::slice::from_raw_parts(ptr, len));
                        loaded += len;
                    },
                )?;
            }
            Ok(headers.summary())
        },
    )?;
    let peak = heap::peak_since(start);
    drop(ring);
    Ok((peak, summary))
}

#[test]
fn large_segment_uses_at_most_one_chunk_and_headers() {
    let size = X86_HEAP + CHUNK + 1;
    let header = header(size, size);
    let fs = SparseElf {
        header,
        size,
        available: size,
    };
    let node = VNode::file();

    let mut loaded = 0;
    let (peak, summary) = load(
        &fs,
        &node,
        || false,
        |offset, bytes| {
            assert_eq!(offset, loaded);
            let mut expected = [0; CHUNK];
            if offset == 0 {
                expected[..header.len()].copy_from_slice(&header);
            }
            if offset + bytes.len() == size {
                expected[bytes.len() - 1] = b'Z';
            }
            assert_eq!(bytes, &expected[..bytes.len()]);
            loaded += bytes.len();
        },
    )
    .unwrap();
    assert_eq!(loaded, size);
    assert!(
        peak <= CHUNK + header.len(),
        "exec buffered {peak} bytes for a {size}-byte segment"
    );
    // Entry, the program headers inside the segment, the page-aligned heap
    // start and image size, and no writable bytes.
    let pages = (size as u64 + 4095) & !4095;
    assert_eq!(summary, [0x4000_0000, 0x4000_0000 + ELF_HEADER as u64, 0x4000_0000 + pages, pages, 0]);
}

#[test]
fn cancelled_exec_and_directories_keep_their_errors() {
    let size = ELF_HEADER + PHDR;
    let fs = SparseElf {
        header: header(size, size),
        size,
        available: size,
    };
    assert_eq!(
        load(&fs, &VNode::file(), || true, |_, _| panic!("cancelled")),
        Err(Errno(EINTR))
    );
    let dir = VNode {
        ino: 2,
        ..VNode::file()
    };
    assert_eq!(
        load(&fs, &dir, || false, |_, _| panic!("directory")),
        Err(Errno(EISDIR))
    );
}

#[test]
fn eof_inside_advertised_segment_is_eio() {
    let size = CHUNK + 100;
    let fs = SparseElf {
        header: header(size, size),
        size,
        available: size - 1,
    };
    assert_eq!(
        load(&fs, &VNode::file(), || false, |_, _| {}),
        Err(Errno(EIO))
    );
}

/// The image the old exec built from its whole-file copy: each PT_LOAD page
/// zero-filled when first mapped, then each segment's file bytes copied to its p_vaddr in
/// program-header order.
fn whole_file_image(data: &[u8]) -> std::collections::BTreeMap<u64, Vec<u8>> {
    let u16_at = |at: usize| u16::from_le_bytes(data[at..at + 2].try_into().unwrap()) as usize;
    let u64_at = |at: usize| u64::from_le_bytes(data[at..at + 8].try_into().unwrap());
    let mut pages = std::collections::BTreeMap::new();
    let phoff = u64_at(32) as usize;
    for index in 0..u16_at(56) {
        let ph = phoff + index * PHDR;
        let (offset, vaddr) = (u64_at(ph + 8) as usize, u64_at(ph + 16));
        let (filesz, memsz) = (u64_at(ph + 32) as usize, u64_at(ph + 40));
        if u32::from_le_bytes(data[ph..ph + 4].try_into().unwrap()) != 1 || memsz == 0 {
            continue;
        }
        for page in ((vaddr & !4095)..vaddr + memsz).step_by(4096) {
            pages.entry(page).or_insert_with(|| vec![0u8; 4096]);
        }
        let mut done = 0;
        while done < filesz {
            let address = vaddr + done as u64;
            let at = (address & 4095) as usize;
            let count = (4096 - at).min(filesz - done);
            pages.get_mut(&(address & !4095)).unwrap()[at..at + count]
                .copy_from_slice(&data[offset + done..offset + done + count]);
            done += count;
        }
    }
    pages
}

/// The image the chunked loader builds by reading the file, bounded reads only,
/// and the number of file bytes it read.
fn chunked_image(path: &std::path::Path, machine: u16, user_end: u64)
    -> (std::collections::BTreeMap<u64, Vec<u8>>, usize)
{
    use std::os::unix::fs::FileExt;
    let file = std::fs::File::open(path).unwrap();
    let size = file.metadata().unwrap().len();
    let mut total = 0;
    let mut read = |offset: u64, bytes: &mut [u8]| {
        assert!(bytes.len() <= CHUNK, "{}: a {}-byte read", path.display(), bytes.len());
        total += bytes.len();
        file.read_exact_at(bytes, offset).map_err(|_| Errno(EIO))
    };
    let mut header = [0; ELF_HEADER];
    read(0, &mut header).unwrap();
    let headers = exec_elf::Headers::read(&header, size, machine, user_end, &mut read)
        .unwrap_or_else(|errno| panic!("{}: refused with {errno:?}", path.display()));
    let mut pages: std::collections::BTreeMap<u64, Box<[u8; 4096]>> = Default::default();
    // SAFETY: each page is a distinct boxed frame, zeroed when first handed
    // out, and the boxes do not move while the map grows.
    unsafe {
        headers.load_segments(
            &mut read,
            |address, _| Ok(pages.entry(address).or_insert_with(|| Box::new([0; 4096])).as_mut_ptr()),
            |_, _| {},
        ).unwrap();
    }
    drop(read);
    (pages.into_iter().map(|(address, page)| (address, page.to_vec())).collect(), total)
}

#[test]
fn staged_programs_load_the_same_bytes_as_the_whole_file_copy() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("userspace/programs/target");
    for (arch, machine, user_end) in [("x86_64", 62, 0x8000_0000_0000), ("aarch64", 183, 0x0001_0000_0000_0000)] {
        let release = root.join(format!("{arch}-breenix/release"));
        let mut programs: Vec<_> = std::fs::read_dir(&release)
            .unwrap_or_else(|_| panic!("{} is missing: build userspace for {arch} first", release.display()))
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                let mut header = [0; 20];
                path.is_file() && std::fs::File::open(path)
                    .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut header)).is_ok()
                    && header[..4] == *b"\x7fELF" && header[16] == 2
                    && u16::from_le_bytes([header[18], header[19]]) == machine
            })
            .collect();
        assert!(!programs.is_empty(), "{}: no {arch} executables", release.display());
        let ordinary = programs.len();
        // llc is larger than the x86 kernel heap; llvm-mc is the control.
        for tool in ["llc", "llvm-mc"] {
            let path = root.join("llvm").join(arch).join(tool);
            assert!(path.is_file(), "{} is missing: build the LLVM tools first", path.display());
            programs.push(path);
        }
        for path in &programs {
            let data = std::fs::read(path).unwrap();
            assert!(
                chunked_image(path, machine, user_end).0 == whole_file_image(&data),
                "{}: loaded segment bytes differ from the whole-file copy",
                path.display()
            );
        }
        println!("{arch}: {ordinary} programs, llc and llvm-mc load identically");
    }
}

/// The processes suite's kill-heap case pads its exec helper with zeros so the
/// exec lasts long enough to be killed inside. exec reads only PT_LOAD bytes,
/// so the padding must be a PT_LOAD of its own: the patched helper loads its
/// program unchanged and reads its padding through the same bounded reads.
#[test]
fn padded_exec_helper_reads_its_padding() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let helper = root.join("userspace/programs/target/aarch64-breenix/release/processes-exec_test");
    let image = std::fs::read(&helper)
        .unwrap_or_else(|_| panic!("{} is missing: build aarch64 userspace first", helper.display()));
    let size = 1 << 20;
    assert!(image.len() * 2 <= size);
    let mut padded = image.clone();
    padded.resize(size, 0);
    let (offset, header) = padded_elf::padding_load(&image, size as u64).unwrap();
    let offset = offset as usize;
    padded[offset..offset + PHDR].copy_from_slice(&header);
    let path = std::env::temp_dir().join(format!("breenix-padded-helper-{}", std::process::id()));
    std::fs::write(&path, &padded).unwrap();
    let (pages, read) = chunked_image(&path, 183, 0x0001_0000_0000_0000);
    std::fs::remove_file(&path).unwrap();
    assert!(pages == whole_file_image(&padded), "the patched helper does not load as its whole-file copy");
    // The program header table is loaded with the first segment, so the one
    // rewritten header shows there; the rest of the program's pages equal
    // the original's.
    let mut program_pages = whole_file_image(&image);
    let slot = 0x4000_0000 + offset as u64;
    assert_eq!(&image[..8], &program_pages[&0x4000_0000][..8], "the first PT_LOAD maps file offset 0");
    program_pages.get_mut(&(slot & !4095)).unwrap()[(slot & 4095) as usize..][..PHDR].copy_from_slice(&header);
    for (address, page) in program_pages {
        assert_eq!(pages[&address], page, "program page {address:#x} changed");
    }
    let padding = size - ((image.len() + 4095) & !4095);
    let (_, program) = chunked_image(&helper, 183, 0x0001_0000_0000_0000);
    assert_eq!(read, program + padding, "exec read {read} bytes of a {size}-byte padded helper");
}
