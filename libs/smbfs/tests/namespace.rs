use libsmb::msg::{CreateDisposition, DirectoryEntry, FileAllInformation, FileId, SetInfo};
use smbfs::{Backend, Namespace, ROOT};
use std::collections::BTreeMap;

#[derive(Default)]
struct Fake {
    files: BTreeMap<String, bool>,
    handles: BTreeMap<u64, String>,
    next: u64,
    queries: usize,
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
    fn open(
        &mut self,
        path: &str,
        _: u32,
        disposition: CreateDisposition,
        _: u32,
    ) -> Result<FileId, u32> {
        if matches!(disposition, CreateDisposition::Create) {
            if self.files.contains_key(path) {
                return Err(0xc0000035);
            }
            self.files.insert(path.into(), true);
        }
        if !self.files.contains_key(path) {
            return Err(0xc0000034);
        }
        self.next += 1;
        self.handles.insert(self.next, path.into());
        Ok(FileId {
            persistent: self.next,
            volatile: 0,
        })
    }
    fn close(&mut self, id: FileId) -> Result<(), u32> {
        self.handles.remove(&id.persistent);
        Ok(())
    }
    fn query_file(&mut self, id: FileId) -> Result<FileAllInformation, u32> {
        self.queries += 1;
        Ok(info(self.files[&self.handles[&id.persistent]]))
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
                let moved: Vec<_> = self
                    .files
                    .iter()
                    .filter(|(p, _)| *p == &old || p.starts_with(&format!("{old}\\")))
                    .map(|(p, d)| (p.clone(), *d))
                    .collect();
                for (p, d) in moved {
                    self.files.remove(&p);
                    self.files.insert(format!("{name}{}", &p[old.len()..]), d);
                }
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
