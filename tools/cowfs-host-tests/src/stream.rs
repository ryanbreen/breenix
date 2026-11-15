//! Reproducible streams: a create/sparse-write/fsync/remount prefix, followed by
//! xorshift64* choices. Seed 0 maps to 0x9e3779b97f4a7c15; no host entropy or clock.
use crate::{model::Model, *};
use alloc::{format, vec, vec::Vec};

pub struct Stream {
    random: u64,
    index: u64,
    shadow: Model,
}
impl Stream {
    pub fn new(seed: u64) -> Self {
        Self {
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
