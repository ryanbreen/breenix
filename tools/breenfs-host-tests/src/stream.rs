//! Reproducible streams: a create/sparse-write/fsync/remount prefix, followed by
//! xorshift64* choices. Seed 0 maps to 0x9e3779b97f4a7c15; no host entropy or clock.
use crate::{model::Model, *};
use alloc::{format, vec, vec::Vec};

pub struct Stream {
    random: u64,
    stress: bool,
    index: u64,
    shadow: Model,
}
impl Stream {
    pub fn new(seed: u64) -> Self {
        let stress = seed & TORTURE_SEED != 0;
        let seed = seed & !TORTURE_SEED;
        Self {
            stress,
            random: if seed == 0 {
                0x9e37_79b9_7f4a_7c15
            } else {
                seed
            },
            index: 0,
            shadow: Model::new(),
        }
    }
    fn random(&mut self) -> u64 {
        self.random ^= self.random >> 12;
        self.random ^= self.random << 25;
        self.random ^= self.random >> 27;
        self.random.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn pick(&mut self, ids: &[Ino]) -> Ino {
        ids[(self.random() % ids.len() as u64) as usize]
    }
    fn name(&mut self, dir: Ino, state: &State) -> Vec<u8> {
        if let Contents::Directory { entries, .. } = &state.inodes[&dir].contents {
            if !entries.is_empty() && self.random() % 4 != 0 {
                return entries
                    .keys()
                    .nth((self.random() % entries.len() as u64) as usize)
                    .unwrap()
                    .clone();
            }
        }
        format!("n{}", self.index).into_bytes()
    }
    // After the historical eight-operation corpus, keep a file alive through
    // split-sized xattrs, overwrite/free cycles and enough commits to wrap the
    // superblock slots. Random suffix operations still vary namespace shapes.
    fn stress(&mut self) -> Action {
        let round = self.index / 16;
        let name = format!("stress-{round}").into_bytes();
        let state = self.shadow.state().expect("model state");
        let ino = match &state.inodes[&ROOT].contents {
            Contents::Directory { entries, .. } => entries.get(&name).copied(),
            _ => unreachable!(),
        };
        let Some(ino) = ino else {
            return Action::Create {
                dir: ROOT,
                name,
                mode: 0o644,
            };
        };
        match self.index % 16 {
            1..=4 => Action::Setxattr {
                ino,
                name: format!("user.stress-{}", self.index % 16).into_bytes(),
                value: vec![(self.random() >> 56) as u8; 4096],
            },
            5 | 7 => Action::Write {
                ino,
                offset: 0,
                bytes: vec![(self.random() >> 56) as u8; (2 + self.random() as usize % 7) * 4096],
            },
            9 => Action::Truncate { ino, size: 0 },
            11 => Action::Unlink { dir: ROOT, name },
            13 => Action::Remount,
            _ => Action::Fsync { ino },
        }
    }
    fn generated(&mut self) -> Action {
        let state = self.shadow.state().expect("model state");
        let ids: Vec<_> = state.inodes.keys().copied().collect();
        let dirs: Vec<_> = ids
            .iter()
            .copied()
            .filter(|i| matches!(state.inodes[i].contents, Contents::Directory { .. }))
            .collect();
        let mut files: Vec<_> = ids
            .iter()
            .copied()
            .filter(|i| matches!(state.inodes[i].contents, Contents::File { .. }))
            .collect();
        if files.is_empty() {
            files.push(16);
        } // Deliberately exercises ENOENT after unlink.
        let dir = self.pick(&dirs);
        let ino = self.pick(&files);
        let name = self.name(dir, &state);
        let mode = 0o600 | (self.random() as u32 & 0o177);
        let offset = [0, 4095, 4096, 8193, 1 << 32][(self.random() % 5) as usize];
        match self.random() % 14 {
            0 => Action::Create { dir, name, mode },
            1 => {
                let count = (self.random() % 65) as usize;
                let bytes = (0..count).map(|_| (self.random() >> 56) as u8).collect();
                Action::Write { ino, offset, bytes }
            }
            2 => Action::Truncate { ino, size: offset },
            3 => Action::Link {
                ino: self.pick(&ids),
                dir,
                name,
            },
            4 => Action::Unlink { dir, name },
            5 => {
                let to_dir = self.pick(&dirs);
                let to = self.name(to_dir, &state);
                Action::Rename {
                    from_dir: dir,
                    from: name,
                    to_dir,
                    to,
                }
            }
            6 => Action::Mkdir { dir, name, mode },
            7 => Action::Rmdir { dir, name },
            8 => Action::Symlink {
                dir,
                name,
                target: vec![b'.', b'/', b't'],
            },
            9 => Action::Setattr {
                ino: self.pick(&ids),
                attrs: SetAttr {
                    mode: Some(mode),
                    uid: Some((self.random() % 8) as u32),
                    gid: Some((self.random() % 8) as u32),
                    atime: Some(Time {
                        seconds: self.index as i64,
                        nanos: 123,
                    }),
                    mtime: None,
                },
            },
            10 => Action::Setxattr {
                ino: self.pick(&ids),
                name: b"user.model".to_vec(),
                value: self.random().to_le_bytes().to_vec(),
            },
            11 => Action::Removexattr {
                ino: self.pick(&ids),
                name: b"user.model".to_vec(),
            },
            12 => Action::Fsync {
                ino: self.pick(&ids),
            },
            _ => Action::Remount,
        }
    }
}
impl Iterator for Stream {
    type Item = Operation;
    fn next(&mut self) -> Option<Self::Item> {
        let action = match self.index {
            0 => Action::Create {
                dir: ROOT,
                name: b"seed-file".to_vec(),
                mode: 0o644,
            },
            1 => Action::Write {
                ino: 16,
                offset: 8193,
                bytes: b"seed-data".to_vec(),
            },
            2 => Action::Fsync { ino: 16 },
            3 => Action::Remount,
            _ if self.stress && self.index >= 8 && self.index % 16 < 14 => self.stress(),
            _ => self.generated(),
        };
        self.index += 1;
        let op = Operation {
            at: Time {
                seconds: self.index as i64,
                nanos: 0,
            },
            action,
        };
        // Failed operations intentionally remain in the stream to compare errno.
        let _result = self.shadow.execute(&op);
        Some(op)
    }
}

/// Operation budget used by the long-running crash driver.
pub fn torture_operations(seed: u64) -> usize {
    32 + (seed as usize % 4) * 8
}

// The workload tag stays in emitted regression rows, independent of their
// minimized operation count. Ordinary model/minimizer seeds retain their stream.
const TORTURE_SEED: u64 = 1 << 63;
pub fn torture_seed(seed: u64) -> u64 {
    seed | TORTURE_SEED
}

#[derive(Clone, Copy)]
enum FillPhase {
    Setup(usize),
    SetupSync,
    Write,
    WriteSync,
    Delete(usize),
    DeleteSync,
    Remount,
    ReuseSync,
    Done,
}

/// Three deterministic fill/delete cycles, with feedback from the filesystem.
/// Call complete with the result of each operation before requesting another.
/// Successful writes are synced before the next attempt, so an ENOSPC comparison
/// can inspect the committed extents without committing the refused operation.
pub struct FillDelete {
    random: u64,
    phase: FillPhase,
    round: u64,
    index: i64,
    files: [Ino; 4],
    sizes: [u64; 4],
    order: Vec<usize>,
    directories: [usize; 6],
    target: usize,
    chunk: usize,
}

impl FillDelete {
    const CHUNKS: [usize; 6] = [
        4 * 1024 * 1024,
        1024 * 1024,
        256 * 1024,
        64 * 1024,
        16 * 1024,
        4096,
    ];

    pub fn new(seed: u64) -> Self {
        let mut stream = Self {
            random: if seed == 0 {
                0x9e37_79b9_7f4a_7c15
            } else {
                seed
            },
            phase: FillPhase::Setup(0),
            round: 0,
            index: 0,
            files: [0; 4],
            sizes: [0; 4],
            order: vec![0, 1, 2, 3],
            directories: [0, 1, 2, 3, 4, 5],
            target: 0,
            chunk: 0,
        };
        stream.shuffle();
        for i in (1..6).rev() {
            let j = stream.random() as usize % (i + 1);
            stream.directories.swap(i, j);
        }
        stream
    }

    fn random(&mut self) -> u64 {
        self.random ^= self.random >> 12;
        self.random ^= self.random << 25;
        self.random ^= self.random >> 27;
        self.random.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn shuffle(&mut self) {
        for i in (1..self.order.len()).rev() {
            let j = self.random() as usize % (i + 1);
            self.order.swap(i, j);
        }
    }

    fn name(&self, directory: bool, slot: usize) -> Vec<u8> {
        format!("fill-{}-{slot}", if directory { "dir" } else { "file" }).into_bytes()
    }

    pub fn operation(&mut self) -> Option<Operation> {
        let action = match self.phase {
            FillPhase::Setup(slot) if slot < 4 => Action::Create {
                dir: ROOT,
                name: self.name(false, slot),
                mode: 0o644,
            },
            FillPhase::Setup(slot) => Action::Mkdir {
                dir: ROOT,
                name: self.name(true, slot - 4),
                mode: 0o755,
            },
            FillPhase::SetupSync
            | FillPhase::WriteSync
            | FillPhase::DeleteSync
            | FillPhase::ReuseSync => Action::Fsync { ino: ROOT },
            FillPhase::Write => {
                let choice = self.random() as usize % self.order.len();
                self.target = self.order[choice];
                let mut bytes = vec![0; Self::CHUNKS[self.chunk]];
                // Allocated data with sparse nonzero markers keeps the byte-map
                // oracle small while checking page placement and payload identity.
                for page in bytes.chunks_mut(4096) {
                    let value = self.random();
                    page[(value as usize >> 8) % 4096] = (value as u8).max(1);
                }
                Action::Write {
                    ino: self.files[self.target],
                    offset: self.sizes[self.target],
                    bytes,
                }
            }
            FillPhase::Delete(0) => Action::Unlink {
                dir: ROOT,
                name: self.name(false, self.order[0]),
            },
            FillPhase::Delete(1) => Action::Rmdir {
                dir: ROOT,
                name: self.name(true, self.directories[self.round as usize * 2]),
            },
            FillPhase::Delete(2) => Action::Truncate {
                ino: self.files[self.order[1]],
                size: 0,
            },
            FillPhase::Delete(3) => Action::Rmdir {
                dir: ROOT,
                name: self.name(true, self.directories[self.round as usize * 2 + 1]),
            },
            FillPhase::Delete(_) => unreachable!("four freeing calls"),
            FillPhase::Remount => Action::Remount,
            FillPhase::Done => return None,
        };
        self.index += 1;
        Some(Operation {
            at: Time {
                seconds: self.index,
                nanos: 0,
            },
            action,
        })
    }

    pub fn complete(&mut self, result: Result<Reply>) {
        self.phase = match (self.phase, result) {
            (FillPhase::Setup(slot), Ok(Reply::Inode(ino))) => {
                if slot < 4 {
                    self.files[slot] = ino;
                }
                if slot == 9 {
                    FillPhase::SetupSync
                } else {
                    FillPhase::Setup(slot + 1)
                }
            }
            (FillPhase::SetupSync | FillPhase::WriteSync, Ok(Reply::Done)) => FillPhase::Write,
            (FillPhase::Write, Ok(Reply::Written(count))) => {
                assert_eq!(count, Self::CHUNKS[self.chunk]);
                self.sizes[self.target] += count as u64;
                FillPhase::WriteSync
            }
            (FillPhase::Write, Err(Error::Errno(Errno::ENOSPC))) => {
                self.chunk += 1;
                if self.chunk == Self::CHUNKS.len() {
                    FillPhase::Delete(0)
                } else {
                    FillPhase::Write
                }
            }
            (FillPhase::Delete(slot), Ok(Reply::Done)) => {
                if slot == 2 {
                    self.sizes[self.order[1]] = 0;
                }
                if slot == 3 {
                    FillPhase::DeleteSync
                } else {
                    FillPhase::Delete(slot + 1)
                }
            }
            (FillPhase::DeleteSync, Ok(Reply::Done)) => FillPhase::Remount,
            (FillPhase::Remount, Ok(Reply::Done)) => {
                self.round += 1;
                if self.round == 3 {
                    FillPhase::Done
                } else {
                    // Deletion's G and remount's G+1 protect the released data.
                    // Publish its promotion to FREE before the next admission.
                    FillPhase::ReuseSync
                }
            }
            (FillPhase::ReuseSync, Ok(Reply::Done)) => {
                self.chunk = 0;
                self.order.remove(0);
                self.shuffle();
                FillPhase::Write
            }
            _ => panic!("unexpected fill/delete result: {result:?}"),
        };
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use crate::{recording::RecordingDevice, WriterAdapter};
    use alloc::sync::Arc;
    use std::time::Instant;

    #[test]
    fn snapshot_seed_matches_all_live_roots() {
        let started = Instant::now();
        let seed = snapshot_seed(0x2414);
        let operations: Vec<_> = Stream::new(seed).take(32).collect();
        assert_eq!(operations, Stream::new(seed).take(32).collect::<Vec<_>>());
        assert!(operations.iter().any(|op| matches!(
            op.action,
            Action::Snapshot {
                read_only: true,
                ..
            }
        )));
        assert!(operations.iter().any(|op| matches!(
            op.action,
            Action::Snapshot {
                read_only: false,
                ..
            }
        )));
        assert!(operations
            .iter()
            .any(|op| matches!(op.action, Action::Clone { .. })));
        assert!(operations
            .iter()
            .any(|op| matches!(op.action, Action::DeleteSnapshot { .. })));
        let device = Arc::new(RecordingDevice::new(crate::test_image::image()).unwrap());
        let mut writer = WriterAdapter::new(device.clone()).unwrap();
        let mut model = Model::new();
        let mut frozen = None;
        for (index, op) in operations.iter().enumerate() {
            assert_eq!(writer.execute(op), model.execute(op), "op={index}: {op:?}");
            assert!(
                compare(&model, &writer).unwrap().is_empty(),
                "op={index}: {op:?}"
            );
            let roots = writer.roots().unwrap();
            if index == 14 {
                frozen = Some(roots[b"frozen".as_slice()].state.clone());
            }
            if index == 17 {
                let frozen = frozen.as_ref().unwrap();
                assert_eq!(&roots[b"frozen".as_slice()].state, frozen);
                assert_eq!(&roots[b"clone".as_slice()].state, frozen);
                assert_eq!(&roots[b"writable".as_slice()].state, frozen);
                assert_ne!(&roots[b"default".as_slice()].state, frozen);
                for fs in [&mut model as &mut dyn Fs, &mut writer as &mut dyn Fs] {
                    fs.select_root(b"clone").unwrap();
                    fs.execute(&Operation {
                        at: op.at,
                        action: Action::Write {
                            ino: 16,
                            offset: 0,
                            bytes: b"clone diverges".to_vec(),
                        },
                    })
                    .unwrap();
                    fs.execute(&Operation {
                        at: op.at,
                        action: Action::Create {
                            dir: ROOT,
                            name: b"clone-only".to_vec(),
                            mode: 0o600,
                        },
                    })
                    .unwrap();
                    fs.select_root(b"frozen").unwrap();
                    assert_eq!(
                        fs.execute(&Operation {
                            at: op.at,
                            action: Action::Write {
                                ino: 16,
                                offset: 0,
                                bytes: b"refused".to_vec(),
                            }
                        }),
                        Err(Errno::EROFS.into())
                    );
                    fs.select_root(b"default").unwrap();
                }
                assert!(compare(&model, &writer).unwrap().is_empty());
                let roots = writer.roots().unwrap();
                assert_ne!(
                    roots[b"clone".as_slice()].state,
                    roots[b"default".as_slice()].state
                );
                assert_eq!(&roots[b"frozen".as_slice()].state, frozen);
                assert_eq!(&roots[b"writable".as_slice()].state, frozen);
            }
        }
        let remount = Operation {
            at: Time::default(),
            action: Action::Remount,
        };
        assert_eq!(writer.execute(&remount), model.execute(&remount));
        assert!(compare(&model, &writer).unwrap().is_empty());
        crate::test_image::check(&device.durable_image(), "snapshot-stream");
        assert!(started.elapsed().as_secs_f64() < 60.0);
    }
}
