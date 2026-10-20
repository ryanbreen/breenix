//! Interrupt persisted operations in the manager's actual slot transaction and recover on startup.
#[path = "../scripts/metal/efi-stub/src/slot_transaction.rs"]
mod transaction;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use transaction::{Store, begin, recover};

struct Disk {
    files: RefCell<BTreeMap<String, Vec<u8>>>,
    remaining: Cell<usize>,
    after: bool,
}
impl Disk {
    fn mutation(&self, change: impl FnOnce(&mut BTreeMap<String, Vec<u8>>)) -> Result<(), ()> {
        let count = self.remaining.get();
        if count == 0 && !self.after { return Err(()); }
        change(&mut self.files.borrow_mut());
        if count == 0 { return Err(()); }
        self.remaining.set(count - 1);
        Ok(())
    }
}
impl Store for Disk {
    type Error = ();
    fn exists(&self, name: &str) -> bool { self.files.borrow().contains_key(name) }
    fn read(&self, name: &str, bytes: &mut [u8]) -> Result<usize, ()> {
        let files = self.files.borrow();
        let data = files.get(name).ok_or(())?;
        if data.len() > bytes.len() { return Err(()); }
        bytes[..data.len()].copy_from_slice(data);
        Ok(data.len())
    }
    fn write(&self, name: &str, bytes: &[u8]) -> Result<(), ()> {
        self.mutation(|files| { files.insert(name.into(), bytes.into()); })
    }
    fn delete(&self, name: &str) -> Result<(), ()> {
        self.mutation(|files| { files.remove(name); })
    }
    fn rename(&self, from: &str, to: &str) -> Result<(), ()> {
        assert!(!self.exists(to));
        self.mutation(|files| { let data = files.remove(from).unwrap(); files.insert(to.into(), data); })
    }
    fn invalid(&self) {}
}

#[test]
fn interrupted_commit_and_rollback_recover_matching_image_note_pairs() {
    for rollback in [false, true] {
        for first_install in [false, true] {
            if rollback && first_install { continue; }
            for after in [false, true] {
                for fault in 0..22 {
                    let mut initial = BTreeMap::new();
                    for (name, data) in [("CURRENT", "old"), ("PREVIOUS", "older"), ("NEXT", "new")] {
                        if first_install && name != "NEXT" { continue; }
                        for extension in ["IMG", "TXT"] {
                            initial.insert(format!("{name}.{extension}"), data.as_bytes().to_vec());
                        }
                    }
                    let disk = Disk { files: RefCell::new(initial), remaining: Cell::new(fault), after };
                    let _ = begin(&disk, rollback);
                    disk.remaining.set(usize::MAX);
                    recover(&disk).unwrap();
                    let files = disk.files.borrow();
                    for name in ["CURRENT", "PREVIOUS", "NEXT", "SWAP"] {
                        assert_eq!(files.get(&format!("{name}.IMG")), files.get(&format!("{name}.TXT")),
                                   "rollback={rollback} first={first_install} after={after} fault={fault} {name}");
                    }
                    let current = files.get("CURRENT.IMG").map(Vec::as_slice);
                    assert!(current.is_some() || (first_install && files.contains_key("NEXT.IMG")));
                    if rollback {
                        assert!(current == Some(b"old".as_slice()) || current == Some(b"older".as_slice()));
                    } else {
                        assert!(current == Some(b"old".as_slice()) || current == Some(b"new".as_slice()) || first_install);
                    }
                }
            }
        }
    }
}
