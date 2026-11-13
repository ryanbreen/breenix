#!/usr/bin/env python3
"""Exercise the production namespace roots and ancestry with directory fixtures."""
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
source = (ROOT / 'kernel/src/fs/namei.rs').read_text()


def function(name):
    start = source.index('fn ' + name + '(')
    body = source.index('{', start)
    depth = 0
    for end in range(body, len(source)):
        depth += (source[end] == '{') - (source[end] == '}')
        if depth == 0:
            return source[start:end + 1]
    raise ValueError('unterminated function ' + name)


production = '\n'.join(function(name) for name in ('namespace_root', 'top', 'copy_bytes', 'join', 'read_dir_inode', 'ancestry'))
start = source.index('    pub fn is_mount_point(')
end = source.index('\n    }', start) + len('\n    }')
production += '\nimpl Resolved {\n' + source[start:end] + '\n}\n'
harness = r'''
extern crate alloc;
use std::collections::BTreeMap;
const ENOENT: i32 = 2;
const ENOTDIR: i32 = 20;
const ENAMETOOLONG: i32 = 36;
const PATH_MAX: usize = 4096;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mount(usize);
impl Mount {
    fn root() -> Self { Self(0) }
    fn root_inode(self) -> u64 { match self.0 {
        0 => 100, 1 => 2, 2 => 10, 3 => 11, 4 => 20, 5 => 21, _ => unreachable!()
    } }
}
struct Held(ext2::Ext2Fs);
impl Held {
    fn fs(&mut self, _: Mount) -> Result<&ext2::Ext2Fs, u64> { Ok(&self.0) }
}
mod mounts {
    pub fn bind_cover(key: (usize, u64)) -> Option<usize> {
        if key.0 != 0 { return None; }
        match key.1 { 10 => Some(2), 11 => Some(3), 20 => Some(4),
            21 => Some(5), _ => None }
    }
    pub fn attachment(id: usize) -> Option<((usize, u64), Vec<u8>)> {
        let name = match id { 1 => b"home".as_slice(), 2 => b"etc", 3 => b"root",
            4 => b"var", 5 => b"run", _ => return None };
        Some(((0, 100), name.to_vec()))
    }
    pub fn is_mount_root(id: usize, ino: u64) -> bool {
        [(0, 100), (0, 22), (0, 10), (0, 11), (0, 20), (0, 21),
            (1, 2), (2, 10), (3, 11), (4, 20), (5, 21)].contains(&(id, ino))
    }
    pub fn path_is_mount(path: &[u8]) -> bool { path == b"/proc" || path == b"/dev" }
}
mod ufs_abi { pub mod payload { pub const ROOT: u64 = 1; } }
fn ino32(ino: u64) -> Result<u32, u64> { ino.try_into().map_err(|_| 75u64) }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Node { Ext2(Mount, u64), Virtual }
pub struct Frame { name: Vec<u8>, node: Node }
pub enum Target { Inode { mount: Mount, ino: u64 }, Virtual, Absent {}, User { node: u64 } }
pub struct Resolved { target: Target, path: alloc::boxed::Box<[u8]> }
fn ext2_errno(_: &'static str) -> u64 { 5 }
mod ext2 {
    use super::*;
    pub struct Ext2Inode { pub ino: u32, pub i_links_count: u16 }
    impl Ext2Inode { pub fn is_dir(&self) -> bool { true } }
    pub struct Ext2Fs(pub BTreeMap<u32, Vec<u8>>);
    impl Ext2Fs {
        pub fn read_inode(&self, ino: u32) -> Result<Ext2Inode, &'static str> {
            self.0.get(&ino).ok_or("inode")?;
            Ok(Ext2Inode { ino, i_links_count: 2 })
        }
        pub fn read_directory(&self, inode: &Ext2Inode) -> Result<Vec<u8>, &'static str> {
            self.0.get(&inode.ino).cloned().ok_or("directory")
        }
    }
    pub mod dir {
        pub struct Entry<'a> { pub inode: u32, pub name: &'a [u8] }
        pub fn raw_entries(mut data: &[u8]) -> impl Iterator<Item=Entry<'_>> {
            std::iter::from_fn(move || {
                if data.is_empty() { return None; }
                let inode = u32::from_le_bytes(data[..4].try_into().unwrap());
                let len = data[4] as usize;
                let name = &data[5..5+len];
                data = &data[5+len..];
                Some(Entry { inode, name })
            })
        }
        pub fn find_inode(data: &[u8], name: &str) -> Option<u32> {
            raw_entries(data).find(|entry| entry.name == name.as_bytes()).map(|entry| entry.inode)
        }
    }
}
fn fixture() -> ext2::Ext2Fs {
    let directories = [
        (2, vec![(2, ".."), (10, "etc"), (11, "root"), (100, "physical-generation")]),
        (100, vec![(2, ".."), (200, "bin"), (22, "home")]),
        (22, vec![(100, "..")]),
        (20, vec![(90, "..")]),
        (21, vec![(90, "..")]),
        (200, vec![(100, ".."), (201, "deep")]),
        (201, vec![(200, "..")]),
        (10, vec![(2, ".."), (12, "config")]),
        (11, vec![(2, ".."), (13, "data")]),
        (12, vec![(10, "..")]), (13, vec![(11, "..")]),
    ];
    ext2::Ext2Fs(directories.into_iter().map(|(ino, entries)| {
        let mut data = vec![];
        for (child, name) in entries {
            data.extend_from_slice(&u32::to_le_bytes(child));
            data.push(name.len() as u8);
            data.extend_from_slice(name.as_bytes());
        }
        (ino, data)
    }).collect())
}
#[test]
fn generation_and_shared_directory_ancestry_stay_in_the_selected_namespace() {
    let mut held = Held(fixture());
    assert_eq!(top(&[]), Node::Ext2(Mount(0), 100));
    assert_eq!(join(&ancestry(&mut held, Mount(0), 100).unwrap()).unwrap(), b"/");
    assert_eq!(join(&ancestry(&mut held, Mount(0), 201).unwrap()).unwrap(), b"/bin/deep");
    assert_eq!(join(&ancestry(&mut held, Mount(2), 12).unwrap()).unwrap(), b"/etc/config");
    assert_eq!(join(&ancestry(&mut held, Mount(3), 13).unwrap()).unwrap(), b"/root/data");
    assert_eq!(join(&ancestry(&mut held, Mount(4), 20).unwrap()).unwrap(), b"/var");
    assert_eq!(join(&ancestry(&mut held, Mount(5), 21).unwrap()).unwrap(), b"/run");
    assert_eq!(join(&ancestry(&mut held, Mount(1), 2).unwrap()).unwrap(), b"/home");
}
#[test]
fn directory_descriptor_storage_identity_recovers_the_bind_ancestry() {
    // DirectoryFile retains the storage mount id until the open-file migration.
    // fchdir/openat must recover the namespace bind from that inode identity.
    let mut held = Held(fixture());
    assert_eq!(join(&ancestry(&mut held, Mount(0), 12).unwrap()).unwrap(), b"/etc/config");
    assert_eq!(join(&ancestry(&mut held, Mount(0), 13).unwrap()).unwrap(), b"/root/data");
}
#[test]
fn shared_roots_are_mount_points_and_cannot_be_renamed_as_placeholders() {
    for (mount, ino, expected) in [(Mount(0), 100, true), (Mount(0), 10, true),
        (Mount(0), 11, true), (Mount(0), 20, true), (Mount(0), 21, true), (Mount(0), 200, false), (Mount(1), 2, true), (Mount(1), 10, false)] {
        assert_eq!(Resolved { target: Target::Inode { mount, ino }, path: Box::new([]) }.is_mount_point(), expected);
    }
    assert!(Resolved { target: Target::Virtual, path: b"/proc".as_slice().into() }.is_mount_point());
    assert!(!Resolved { target: Target::Absent {}, path: b"/missing".as_slice().into() }.is_mount_point());
}
'''

with tempfile.TemporaryDirectory(prefix='breenix-persistent-namespace-') as directory:
    directory = Path(directory)
    test = directory / 'tests.rs'
    test.write_text('#[path = "' + str(ROOT / 'kernel/src/fs/ext2/generation.rs') + '"]\n'
                    + 'pub mod generation;\n' + harness + production)
    binary = directory / 'tests'
    environment = dict(os.environ, CARGO_BUILD_JOBS='4')
    subprocess.run([str(ROOT / 'scripts/build-background.sh'), 'rustc', '--edition=2021', '--test', str(test),
                    '-o', str(binary)], cwd=ROOT, env=environment, check=True)
    subprocess.run([str(binary)], check=True)
