//! Allocation-safe metadata for the devfs directory stream.
use alloc::vec::Vec;

pub const INPUT_DIRECTORY_INO: u64 = 42;
pub const SHM_DIRECTORY_INO: u64 = 0x1000;
pub const MQUEUE_DIRECTORY_INO: u64 = 0x1001;
pub const SHM_DIRECTORY_MODE: u32 = 0o041777;

#[derive(Debug, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub name: &'static str,
    pub ino: u64,
    pub kind: u8,
}

pub fn entries(
    devices: impl ExactSizeIterator<Item = (&'static str, u64)>,
) -> Result<Vec<DirectoryEntry>, u64> {
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(devices.len() + 2)
        .map_err(|_| 12u64)?;
    for (name, ino) in devices {
        entries.push(if name == "input/events" {
            DirectoryEntry {
                name: "input",
                ino: INPUT_DIRECTORY_INO,
                kind: 4,
            }
        } else {
            DirectoryEntry { name, ino, kind: 2 }
        });
    }
    entries.push(DirectoryEntry {
        name: "shm",
        ino: SHM_DIRECTORY_INO,
        kind: 4,
    });
    entries.push(DirectoryEntry {
        name: "mqueue",
        ino: MQUEUE_DIRECTORY_INO,
        kind: 4,
    });
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shm_is_listed_once_as_a_distinct_directory() {
        let entries = entries(
            [
                ("null", 1),
                ("zero", 2),
                ("console", 3),
                ("tty", 4),
                ("rtc0", 5),
            ]
            .into_iter(),
        )
        .unwrap();
        let shm = entries
            .iter()
            .filter(|entry| entry.name == "shm")
            .collect::<Vec<_>>();
        assert_eq!(shm.len(), 1);
        assert_eq!(shm[0].kind, 4);
        let mq = entries.iter().find(|entry| entry.name == "mqueue").unwrap();
        assert_eq!((mq.ino, mq.kind), (MQUEUE_DIRECTORY_INO, 4));
        assert_ne!(mq.ino, shm[0].ino);
        assert_eq!(SHM_DIRECTORY_MODE & 0o170000, 0o040000);
        assert_eq!(SHM_DIRECTORY_MODE & 0o7777, 0o1777);
        assert!(entries
            .iter()
            .filter(|entry| entry.name != "shm" && entry.name != "mqueue")
            .all(|entry| entry.ino != shm[0].ino && entry.kind == 2));
    }
}
