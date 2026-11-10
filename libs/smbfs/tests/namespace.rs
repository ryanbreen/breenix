use libsmb::msg::{
    CreateDisposition, DirectoryEntry, FileAllInformation, FileFsFullSizeInformation, FileId,
    SetInfo,
};
use smbfs::{Backend, Namespace, ROOT};
use std::collections::BTreeMap;

#[derive(Default)]
struct Fake {
    files: BTreeMap<String, bool>,
    handles: BTreeMap<u64, String>,
    access: BTreeMap<u64, u32>,
    next: u64,
    queries: usize,
    data: BTreeMap<String, Vec<u8>>,
    closes: usize,
    flushes: usize,
    writes: Vec<(u64, usize)>,
    basic: Option<(u64, u64, u64, u64, u32)>,
    reconnects: usize,
    drop_read: bool,
    fail_reopen: Option<usize>,
    reopened: usize,
    fail_retry: bool,
    deadline: Option<std::time::Instant>,
}
fn info(dir: bool) -> FileAllInformation {
    FileAllInformation {
        creation_time: 0,
        last_access_time: 116444736000000001,
        last_write_time: 116444736000000002,
        change_time: 0,
        attributes: 0,
        allocation_size: 0,
        end_of_file: 42,
        links: 1,
        delete_pending: false,
        directory: dir,
        index_number: 0,
        ea_size: 0,
        access: 0,
        position: 0,
        mode: 0,
        alignment: 0,
        name: String::new(),
    }
}
impl Backend for Fake {
    fn set_deadline(&mut self, deadline: std::time::Instant) -> Result<(), u32> {
        assert!(deadline > std::time::Instant::now());
        assert!(
            deadline.duration_since(std::time::Instant::now()) < std::time::Duration::from_secs(5)
        );
        self.deadline = Some(deadline);
        Ok(())
    }
    fn reconnect(&mut self, deadline: std::time::Instant) -> Result<(), u32> {
        assert_eq!(self.deadline, Some(deadline));
        self.reconnects += 1;
        self.handles.clear();
        self.access.clear();
        Ok(())
    }
    fn read_at(&mut self, id: FileId, offset: u64, length: usize) -> Result<Vec<u8>, u32> {
        assert!(self.handles.contains_key(&id.persistent), "stale read");
        if self.drop_read || (self.reconnects > 0 && self.fail_retry) {
            self.drop_read = false;
            return Err(0xffff_ffff);
        }
        assert!(length <= ufs_abi::MAX_DATA);
        let data = self.data.get(&self.handles[&id.persistent]).unwrap();
        let at = (offset as usize).min(data.len());
        Ok(data[at..(at + length).min(data.len())].to_vec())
    }
    fn write_at(&mut self, id: FileId, offset: u64, bytes: &[u8]) -> Result<(), u32> {
        assert!(bytes.len() <= ufs_abi::payload::MAX_WRITE);
        self.writes.push((offset, bytes.len()));
        let data = self
            .data
            .entry(self.handles[&id.persistent].clone())
            .or_default();
        data.resize(data.len().max(offset as usize + bytes.len()), 0);
        data[offset as usize..offset as usize + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }
    fn flush(&mut self, id: FileId) -> Result<(), u32> {
        assert!(self.handles.contains_key(&id.persistent), "stale flush");
        // MS-SMB2 3.3.5.11 requires write/append or add-file/add-subdirectory rights.
        if self.access[&id.persistent] & 6 == 0 {
            return Err(0xc0000022);
        }
        self.flushes += 1;
        Ok(())
    }
    fn query_fs_size(&mut self, id: FileId) -> Result<FileFsFullSizeInformation, u32> {
        assert!(self.handles.contains_key(&id.persistent));
        Ok(FileFsFullSizeInformation {
            total_allocation_units: 1000,
            caller_available_allocation_units: 300,
            actual_available_allocation_units: 400,
            sectors_per_allocation_unit: 8,
            bytes_per_sector: 512,
        })
    }
    fn open(
        &mut self,
        path: &str,
        access: u32,
        disposition: CreateDisposition,
        options: u32,
    ) -> Result<FileId, u32> {
        if self.reconnects > 0 {
            assert!(matches!(disposition, CreateDisposition::Open));
            self.reopened += 1;
            if self.fail_reopen == Some(self.reopened) {
                return Err(0xffff_ffff);
            }
        }
        if matches!(disposition, CreateDisposition::Create) {
            if self.files.contains_key(path) {
                return Err(0xc0000035);
            }
            self.files.insert(
                path.into(),
                options & libsmb::msg::create_options::DIRECTORY_FILE != 0,
            );
        }
        if !self.files.contains_key(path)
            && !matches!(
                disposition,
                CreateDisposition::OpenIf | CreateDisposition::OverwriteIf
            )
        {
            return Err(0xc0000034);
        }
        if matches!(
            disposition,
            CreateDisposition::OpenIf | CreateDisposition::OverwriteIf
        ) {
            self.files.entry(path.into()).or_insert(false);
        }
        if matches!(
            disposition,
            CreateDisposition::Overwrite | CreateDisposition::OverwriteIf
        ) {
            self.data.insert(path.into(), Vec::new());
        }
        self.next += 1;
        self.handles.insert(self.next, path.into());
        self.access.insert(self.next, access);
        Ok(FileId {
            persistent: self.next,
            volatile: 0,
        })
    }
    fn close(&mut self, id: FileId) -> Result<(), u32> {
        assert!(self.handles.remove(&id.persistent).is_some(), "stale close");
        self.access.remove(&id.persistent);
        self.closes += 1;
        Ok(())
    }
    fn query_file(&mut self, id: FileId) -> Result<FileAllInformation, u32> {
        self.queries += 1;
        let path = &self.handles[&id.persistent];
        let mut i = info(self.files[path]);
        if let Some(data) = self.data.get(path) {
            i.end_of_file = data.len() as u64;
        }
        Ok(i)
    }
    fn list_handle(&mut self, id: FileId) -> Result<Vec<DirectoryEntry>, u32> {
        let path = &self.handles[&id.persistent];
        let prefix = if path.is_empty() {
            String::new()
        } else {
            format!("{path}\\")
        };
        Ok(self
            .files
            .iter()
            .filter_map(|(name, dir)| {
                let tail = name.strip_prefix(&prefix)?;
                if tail.is_empty() || tail.contains('\\') {
                    return None;
                }
                Some(DirectoryEntry {
                    name: tail.into(),
                    file_id: 0,
                    creation_time: 0,
                    last_access_time: 0,
                    last_write_time: 0,
                    change_time: 0,
                    end_of_file: 0,
                    allocation_size: 0,
                    attributes: if *dir { 16 } else { 0 },
                })
            })
            .collect())
    }
    fn set_info(&mut self, id: FileId, op: SetInfo<'_>) -> Result<(), u32> {
        let old = self.handles[&id.persistent].clone();
        match op {
            SetInfo::Disposition(true) => {
                if self
                    .files
                    .keys()
                    .any(|p| p.starts_with(&format!("{old}\\")))
                {
                    return Err(0xc0000101);
                }
                self.files.remove(&old);
            }
            SetInfo::Rename {
                name,
                replace_if_exists: true,
            } => {
                self.files.remove(name);
                for path in self.handles.values_mut() {
                    if *path == old || path.starts_with(&format!("{old}\\")) {
                        *path = format!("{name}{}", &path[old.len()..]);
                    }
                }
                let moved: Vec<_> = self
                    .files
                    .iter()
                    .filter(|(p, _)| *p == &old || p.starts_with(&format!("{old}\\")))
                    .map(|(p, d)| (p.clone(), *d))
                    .collect();
                for (p, d) in moved {
                    self.files.remove(&p);
                    let new = format!("{name}{}", &p[old.len()..]);
                    if let Some(bytes) = self.data.remove(&p) {
                        self.data.insert(new.clone(), bytes);
                    }
                    self.files.insert(new, d);
                }
            }
            SetInfo::EndOfFile(size) => {
                self.data.entry(old).or_default().resize(size as usize, 0);
            }
            SetInfo::Basic {
                creation_time,
                last_access_time,
                last_write_time,
                change_time,
                attributes,
            } => {
                self.basic = Some((
                    creation_time,
                    last_access_time,
                    last_write_time,
                    change_time,
                    attributes,
                ));
            }
            _ => unreachable!(),
        }
        Ok(())
    }
}
fn setup() -> Namespace<Fake> {
    let mut b = Fake::default();
    for (p, d) in [
        ("", true),
        ("dir", true),
        ("dir\\child", true),
        ("dir\\child\\file", false),
        ("target", true),
    ] {
        b.files.insert(p.into(), d);
    }
    Namespace::new(b)
}
#[test]
fn nested_lookup_listing_and_cache() {
    let mut fs = setup();
    let dir = fs.lookup(ROOT, "dir", 0).unwrap();
    let child = fs.lookup(dir.node, "child", 0).unwrap();
    let file = fs.lookup(child.node, "file", 0).unwrap();
    assert_eq!(file.size, 42);
    assert_eq!(file.atime_ns, 100);
    assert_eq!(file.mtime_ns, 200);
    assert_eq!(
        fs.parent(file.node).unwrap(),
        (child.node, "file".to_owned())
    );
    assert_eq!(fs.parent(ROOT), Err(2));
    assert_eq!(fs.readdir(child.node).unwrap()[0].name, "file");
    let count = fs.backend().queries;
    fs.getattr(file.node, 999_999_999).unwrap();
    assert_eq!(fs.backend().queries, count);
    fs.mkdir(ROOT, "new", 10).unwrap();
    fs.getattr(file.node, 10).unwrap();
    assert!(fs.backend().queries > count);
    assert!(fs.lookup(ROOT, "bad\\name", 0).is_err());
    assert!(fs.lookup(ROOT, "bad:name", 0).is_err());
    assert!(fs.lookup(ROOT, "bad\0name", 0).is_err());
    assert!(fs.lookup(ROOT, &"a".repeat(256), 0).is_err());
}
#[test]
fn mutations_and_rename_rekey_open_subtree() {
    let mut fs = setup();
    let dir = fs.lookup(ROOT, "dir", 0).unwrap();
    let child = fs.lookup(dir.node, "child", 0).unwrap();
    let file = fs.lookup(child.node, "file", 0).unwrap();
    assert_eq!(fs.rmdir(ROOT, "dir"), Err(39));
    let count = fs.backend().queries;
    fs.rename(ROOT, "dir", ROOT, "target").unwrap();
    fs.getattr(file.node, 1).unwrap();
    assert!(fs.backend().queries > count);
    assert_eq!(fs.path(file.node).unwrap(), "target\\child\\file");
    assert_eq!(fs.lookup(child.node, "file", 1).unwrap().node, file.node);
    let count = fs.backend().queries;
    fs.unlink(child.node, "file").unwrap();
    fs.getattr(child.node, 1).unwrap();
    assert!(fs.backend().queries > count);
    let count = fs.backend().queries;
    fs.rmdir(dir.node, "child").unwrap();
    fs.getattr(dir.node, 1).unwrap();
    assert!(fs.backend().queries > count);
    fs.rmdir(ROOT, "target").unwrap();
}

#[test]
fn directory_fsync_obtains_flush_rights_without_changing_the_read_handle() {
    let mut fs = setup();
    let dir = fs.mkdir(ROOT, "save", 0).unwrap();
    let handle = fs.open(dir.node, 0).unwrap();
    let access = fs.backend().access.clone();
    assert!(access.values().all(|mask| mask & 6 == 0));
    let closes = fs.backend().closes;
    fs.fsync(handle).unwrap();
    assert_eq!(fs.backend().flushes, 1);
    assert_eq!(fs.backend().access, access);
    assert_eq!(fs.backend().closes, closes + 1);
    fs.release(handle).unwrap();
}

#[test]
fn file_operations_preserve_bytes_metadata_and_lifetimes() {
    use smbfs::{O_EXCL, O_RDWR, O_TRUNC};
    use ufs_abi::payload::{SetTimes, MAX_WRITE};
    let mut fs = setup();
    let a = fs.create(ROOT, "file", O_EXCL, 0).unwrap();
    assert_eq!(fs.create(ROOT, "file", O_EXCL, 0), Err(17));
    let h = fs.open(a.node, O_RDWR).unwrap();
    let bytes: Vec<_> = (0..MAX_WRITE * 2 + 37).map(|i| (i % 251) as u8).collect();
    assert_eq!(fs.write(h, 19, &bytes).unwrap(), bytes.len());
    assert_eq!(
        fs.backend().writes,
        vec![
            (19, MAX_WRITE),
            (19 + MAX_WRITE as u64, MAX_WRITE),
            (19 + 2 * MAX_WRITE as u64, 37)
        ]
    );
    assert_eq!(fs.read(h, 19, bytes.len()).unwrap(), bytes);
    assert_eq!(fs.read(h, 23, 101).unwrap(), bytes[4..105]);
    assert!(fs.read(h, 19 + bytes.len() as u64, 1).unwrap().is_empty());
    fs.fsync(h).unwrap();
    assert_eq!(fs.backend().flushes, 1);
    let a = fs
        .setattr(
            a.node,
            Some(7),
            Some(SetTimes {
                atime_ns: Some(1200),
                mtime_ns: None,
            }),
            1,
        )
        .unwrap();
    assert_eq!(a.size, 7);
    assert_eq!(fs.backend().basic, Some((0, 116444736000000012, 0, 0, 0)));
    fs.setattr(
        a.node,
        None,
        Some(SetTimes {
            atime_ns: None,
            mtime_ns: Some(3400),
        }),
        2,
    )
    .unwrap();
    assert_eq!(fs.backend().basic, Some((0, 0, 116444736000000034, 0, 0)));
    let st = fs.statfs(ROOT).unwrap();
    assert_eq!(
        (st.bsize, st.blocks, st.bfree, st.bavail, st.namelen),
        (4096, 1000, 400, 300, 255)
    );
    let before = fs.backend().closes;
    fs.release(h).unwrap();
    fs.release(h).unwrap();
    assert_eq!(fs.backend().closes, before + 1);
    let h = fs.open(a.node, O_RDWR | O_TRUNC).unwrap();
    assert!(fs.read(h, 0, 10).unwrap().is_empty());
    fs.release(h).unwrap();
}

#[test]
fn reconnect_reopens_held_paths_without_replaying_create_or_truncate() {
    use smbfs::{O_EXCL, O_RDWR, O_TRUNC};
    let mut fs = setup();
    let a = fs.create(ROOT, "file", O_EXCL, 0).unwrap();
    let h = fs.open(a.node, O_RDWR | O_TRUNC).unwrap();
    fs.write(h, 0, b"saved bytes").unwrap();
    let other = fs.open(a.node, 0).unwrap();
    fs.rename(ROOT, "file", ROOT, "renamed").unwrap();
    fs.backend_mut().drop_read = true;
    assert_eq!(fs.read(h, 0, 11).unwrap(), b"saved bytes");
    assert_eq!(fs.read(other, 6, 5).unwrap(), b"bytes");
    assert_eq!(fs.backend().reconnects, 1);
    assert_eq!(fs.backend().reopened, 2);
    fs.release(h).unwrap();
    fs.release(other).unwrap();
    assert!(fs.backend().handles.is_empty());
}

#[test]
fn second_connection_failure_invalidates_every_file_id() {
    for during_reopen in [true, false] {
        let mut fs = setup();
        let a = fs.create(ROOT, "file", smbfs::O_EXCL, 0).unwrap();
        let h = fs.open(a.node, smbfs::O_RDWR).unwrap();
        let other = fs.open(a.node, 0).unwrap();
        fs.write(h, 0, b"intact").unwrap();
        fs.backend_mut().drop_read = true;
        if during_reopen {
            fs.backend_mut().fail_reopen = Some(2);
        } else {
            fs.backend_mut().fail_retry = true;
        }
        assert_eq!(fs.read(h, 0, 6), Err(5));
        assert_eq!(fs.backend().reconnects, 1);
        let closes = fs.backend().closes;
        fs.release(h).unwrap();
        fs.release(other).unwrap();
        // Check that release discards the failed session's FileIds locally.
        assert_eq!(fs.backend().closes, closes);
        assert_eq!(fs.read(h, 0, 1), Err(9));
    }
}
