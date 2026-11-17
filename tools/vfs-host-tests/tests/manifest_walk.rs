//! Replay the guest's path-based manifest walk over its actual encoded fixture.
use crate::read_conformance::{Disk, NoWait};
use crate::{
    block::cache::BufferCache,
    breenfs::{btree, vfs::BreenFs},
    vfs::*,
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[path = "../../../libs/libbreenix/src/crypto/sha256.rs"]
mod sha256;

fn resolve(fs: &dyn FileSystem, path: &[Vec<u8>]) -> Arc<VNode> {
    let mut node = fs.open(fs.root().unwrap()).unwrap();
    for name in path {
        // namei checks the directory's attributes at each path component.
        assert_eq!(fs.getattr(node.ino).unwrap().kind, NodeKind::Directory);
        node = fs.lookup_open(&node, name).unwrap();
    }
    node
}

// The read suite's 5000-entry fixture takes minutes to build and walk, so the
// default run walks a SAMPLE_ENTRIES fixture from the same script; the opt-in
// breenfs battery runs the full form. The decode and read bounds stay as they
// are: an uncached walk decoded about 18 nodes a row (92524 for 5009 rows), so
// the sample's roughly 500 rows still exceed them without the reuse.
const SAMPLE_ENTRIES: usize = 500;

#[test]
fn manifest_walk_sample_reuses_verified_decoded_nodes() {
    manifest_walk(SAMPLE_ENTRIES, false);
}

#[test]
#[ignore = "5000-entry read fixture (minutes): run by the breenfs battery, or with --ignored when fs/breenfs lookup, readdir, node reads or the decoded-node cache change"]
fn manifest_walk_reuses_verified_decoded_nodes() {
    manifest_walk(5000, true);
}

fn manifest_walk(entries: usize, fixture_from_env: bool) {
    let temporary =
        std::env::temp_dir().join(format!("breenfs-manifest-{entries}-{}", std::process::id()));
    let image = std::env::var_os("BREENFS_READ_FIXTURE")
        .filter(|_| fixture_from_env)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::fs::create_dir(&temporary).unwrap();
            let image = temporary.join("breenfs-read.img");
            assert!(std::process::Command::new("bash")
                .arg(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../scripts/breenfs-read-fixture.sh"
                ))
                .arg(&image)
                .arg(image.with_extension("manifest"))
                .arg(entries.to_string())
                .status()
                .unwrap()
                .success());
            image
        });
    let expected = std::fs::read_to_string(image.with_extension("manifest")).unwrap();
    let disk = Arc::new(Disk {
        bytes: Mutex::new(std::fs::read(&image).unwrap()),
        writable: false,
    });
    let cache = Arc::new(BufferCache::new(NoWait));
    let fs = BreenFs::mount(disk, cache.clone(), 93, 1).unwrap();
    let before = cache.counters();
    btree::measurement::take_decodes();
    let mut todo = vec![Vec::<Vec<u8>>::new()];
    let mut rows = Vec::new();
    while let Some(path) = todo.pop() {
        let node = resolve(fs.as_ref(), &path);
        let attrs = fs.getattr(node.ino).unwrap();
        let mut data = Vec::new();
        match attrs.kind {
            NodeKind::Directory => {
                // read_dir opens the directory independently of lstat.
                let dir = resolve(fs.as_ref(), &path);
                let mut cookie = 0;
                loop {
                    let mut count = 0;
                    let next = fs
                        .readdir(dir.ino, cookie, &mut |entry, _| {
                            if count == 31 {
                                return Ok(false);
                            }
                            count += 1;
                            let mut child = path.clone();
                            child.push(entry.name);
                            todo.push(child);
                            Ok(true)
                        })
                        .unwrap();
                    if count == 0 {
                        break;
                    }
                    cookie = next;
                }
            }
            NodeKind::Regular => {
                let file = resolve(fs.as_ref(), &path);
                let mut buf = [0; 16384];
                loop {
                    let n = fs.read(&file, data.len() as u64, &mut buf).unwrap();
                    if n == 0 {
                        break;
                    }
                    data.extend_from_slice(&buf[..n]);
                }
                assert_eq!(data.len() as u64, attrs.size);
            }
            NodeKind::Symlink => data = fs.readlink(node.ino).unwrap(),
            _ => panic!("unexpected fixture kind"),
        }
        rows.push((path, attrs, data));
    }
    let after = cache.counters();
    let reads = after.hits.count + after.misses.count - before.hits.count - before.misses.count;
    let decodes = btree::measurement::take_decodes();
    println!(
        "manifest rows={} B-tree node reads={reads} decodes={decodes}",
        rows.len()
    );
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(rows.len(), expected.lines().count());
    // Validate the same metadata, contents and hard-link groups as the guest.
    let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let path_bytes = |parts: &[Vec<u8>]| {
        let mut path = vec![b'/'];
        for (i, name) in parts.iter().enumerate() {
            if i != 0 {
                path.push(b'/');
            }
            path.extend(name);
        }
        path
    };
    let mut groups = std::collections::BTreeMap::new();
    for (path, attrs, _) in &rows {
        if attrs.kind != NodeKind::Directory {
            let group = groups
                .entry(attrs.ino)
                .or_insert((0, hex(&path_bytes(path))));
            group.0 += 1;
        }
    }
    let mut actual = String::new();
    use std::fmt::Write;
    for (path, a, data) in rows {
        let kind = match a.kind {
            NodeKind::Directory => "dir",
            NodeKind::Regular => "reg",
            _ => "symlink",
        };
        let digest = if a.kind == NodeKind::Directory {
            "-".into()
        } else {
            hex(&sha256::sha256(&data))
        };
        let group = groups
            .get(&a.ino)
            .filter(|g| g.0 > 1)
            .map(|g| g.1.as_str())
            .unwrap_or("-");
        writeln!(
            actual,
            "{}\t{}\t{:06o}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            hex(&path_bytes(&path)),
            kind,
            a.attrs.mode,
            a.attrs.uid,
            a.attrs.gid,
            a.size,
            a.blocks,
            a.times.mtime.sec,
            a.times.mtime.nsec,
            digest,
            group
        )
        .unwrap();
    }
    assert_eq!(actual, expected);
    // A bounded decoded working set must amortize validation over the walk,
    // rather than decode two whole nodes for each path-component lookup.
    assert!(
        decodes < 2000,
        "manifest walk repeatedly decodes its immutable working set: {decodes}"
    );
    assert!(
        reads < 2000,
        "manifest walk repeatedly copies encoded nodes: {reads}"
    );
    if temporary.exists() {
        std::fs::remove_dir_all(temporary).unwrap();
    }
}
