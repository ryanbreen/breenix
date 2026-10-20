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
const EXT2_ROOT_INO: u32 = 2;
const ENOENT: i32 = 2;
const ENOTDIR: i32 = 20;
const ENAMETOOLONG: i32 = 36;
const PATH_MAX: usize = 4096;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mount { Root, Home }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Node { Ext2(Mount, u32) }
pub struct Frame { name: Vec<u8>, node: Node }
pub enum Target { Inode { mount: Mount, ino: u32 }, Virtual, Absent {} }
pub struct Resolved { target: Target, path: alloc::boxed::Box<[u8]> }
fn fs_errno(_: &'static str) -> u64 { 5 }
mod ext2 {
    use super::*;
    pub fn root_inode() -> u32 { root_namespace().root }
    pub fn root_namespace() -> generation::Namespace { generation::Namespace { root: 100, etc: 10, data: 11 } }
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
        (100, vec![(2, ".."), (200, "bin")]),
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
    let fs = fixture();
    assert_eq!(top(&[]), Node::Ext2(Mount::Root, 100));
    assert_eq!(join(&ancestry(&fs, Mount::Root, 100).unwrap()).unwrap(), b"/");
    assert_eq!(join(&ancestry(&fs, Mount::Root, 201).unwrap()).unwrap(), b"/bin/deep");
    assert_eq!(join(&ancestry(&fs, Mount::Root, 12).unwrap()).unwrap(), b"/etc/config");
    assert_eq!(join(&ancestry(&fs, Mount::Root, 13).unwrap()).unwrap(), b"/root/data");
    assert_eq!(join(&ancestry(&fs, Mount::Home, 2).unwrap()).unwrap(), b"/home");
}
#[test]
fn shared_roots_are_mount_points_and_cannot_be_renamed_as_placeholders() {
    for (mount, ino, expected) in [(Mount::Root, 100, true), (Mount::Root, 10, true),
        (Mount::Root, 11, true), (Mount::Root, 200, false), (Mount::Home, 2, true), (Mount::Home, 10, false)] {
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
