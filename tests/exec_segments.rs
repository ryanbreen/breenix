//! Host checks of the production VFS exec reader with a sparse file and a
//! destination that verifies bytes as they arrive, without a 64 MiB fixture.
#![cfg(test)]
extern crate alloc;

#[path = "../kernel/src/fs/vfs/exec.rs"]
mod exec;
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

fn load(
    fs: &dyn FileSystem,
    node: &VNode,
    killed: impl Fn() -> bool,
    mut copy: impl FnMut(usize, &[u8]),
) -> Result<usize, Errno> {
    let (image, _) = crate::exec::read_image(
        fs,
        node,
        MountFlags::default(),
        &Credentials::kernel(),
        killed,
    )?;
    let size = u64::from_le_bytes(image[96..104].try_into().unwrap()) as usize;
    for (index, bytes) in image[..size].chunks(CHUNK).enumerate() {
        copy(index * CHUNK, bytes);
    }
    Ok(image.capacity())
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
    let peak = load(
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
