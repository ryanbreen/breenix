//! One-use host seed on the boot ESP (S43). The file contains exactly 64 raw
//! bytes. No bytes leave this module until the consumed mark has been flushed.

use crate::BOOT_SEED_BYTES;

const CONSUMED_PREFIX: &[u8; 16] = b"BREENIX-CONSUMED";
pub const CONSUMED_MARK: [u8; BOOT_SEED_BYTES] = {
    let mut mark = [0; BOOT_SEED_BYTES];
    let mut i = 0;
    while i < CONSUMED_PREFIX.len() {
        mark[i] = CONSUMED_PREFIX[i];
        i += 1;
    }
    mark
};

/// The loader's regular file, with the same consume operation exercised on host files.
pub trait SeedFile {
    fn read(&mut self, bytes: &mut [u8]) -> Option<usize>;
    fn rewind(&mut self) -> bool;
    fn write(&mut self, bytes: &[u8]) -> bool;
    fn flush(&mut self) -> bool;
}

/// Missing, malformed, consumed, or unwritable files earn no credit. Read one
/// extra byte to reject oversized files too. A failed write/flush withholds the
/// seed; the caller receives bytes after successful consumption.
pub fn consume<F: SeedFile>(file: Option<&mut F>, seed: &mut [u8; BOOT_SEED_BYTES]) -> bool {
    wipe(seed);
    let Some(file) = file else { return false };
    let mut bytes = [0; BOOT_SEED_BYTES + 1];
    let fresh =
        file.read(&mut bytes) == Some(BOOT_SEED_BYTES) && !bytes.starts_with(CONSUMED_PREFIX);
    let consumed = fresh && file.rewind() && file.write(&CONSUMED_MARK) && file.flush();
    if consumed {
        seed.copy_from_slice(&bytes[..BOOT_SEED_BYTES]);
    }
    wipe(&mut bytes);
    consumed
}

fn wipe(bytes: &mut [u8]) {
    for byte in bytes {
        unsafe { core::ptr::write_volatile(byte, 0) };
    }
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::io::{Cursor, Read, Seek, SeekFrom, Write};
    use std::vec::Vec;

    struct File {
        disk: Cursor<Vec<u8>>,
        writable: bool,
        flush_ok: bool,
        flushed: bool,
    }

    impl File {
        fn new(bytes: &[u8]) -> Self {
            Self {
                disk: Cursor::new(bytes.into()),
                writable: true,
                flush_ok: true,
                flushed: false,
            }
        }
    }

    impl SeedFile for File {
        fn read(&mut self, bytes: &mut [u8]) -> Option<usize> {
            self.disk.read(bytes).ok()
        }
        fn rewind(&mut self) -> bool {
            self.disk.seek(SeekFrom::Start(0)).is_ok()
        }
        fn write(&mut self, bytes: &[u8]) -> bool {
            self.writable && self.disk.write_all(bytes).is_ok()
        }
        fn flush(&mut self) -> bool {
            self.flushed = self.flush_ok;
            self.flush_ok
        }
    }

    #[test]
    fn fresh_is_overwritten_and_flushed_before_handoff_and_cannot_be_reused() {
        let mut file = File::new(&[42; BOOT_SEED_BYTES]);
        let mut seed = [0; BOOT_SEED_BYTES];
        assert!(consume(Some(&mut file), &mut seed));
        assert_eq!(seed, [42; BOOT_SEED_BYTES]);
        assert!(file.flushed);
        assert_eq!(file.disk.get_ref().as_slice(), CONSUMED_MARK);
        assert!(file.rewind());
        assert!(!consume(Some(&mut file), &mut seed));
        assert_eq!(seed, [0; BOOT_SEED_BYTES]);
    }

    #[test]
    fn consumed_earns_nothing() {
        let mut file = File::new(&CONSUMED_MARK);
        let mut seed = [99; BOOT_SEED_BYTES];
        assert!(!consume(Some(&mut file), &mut seed));
        assert_eq!(seed, [0; BOOT_SEED_BYTES]);
        assert!(!file.flushed);
    }

    #[test]
    fn missing_earns_nothing() {
        let mut seed = [99; BOOT_SEED_BYTES];
        assert!(!consume::<File>(None, &mut seed));
        assert_eq!(seed, [0; BOOT_SEED_BYTES]);
    }

    #[test]
    fn short_and_oversized_earn_nothing() {
        for len in [0, 1, BOOT_SEED_BYTES - 1, BOOT_SEED_BYTES + 1] {
            let mut file = File::new(&std::vec![42; len]);
            let mut seed = [99; BOOT_SEED_BYTES];
            assert!(!consume(Some(&mut file), &mut seed), "length {len}");
            assert_eq!(seed, [0; BOOT_SEED_BYTES]);
            assert!(!file.flushed);
        }
    }

    #[test]
    fn write_or_flush_failure_earns_nothing() {
        for writable in [false, true] {
            let mut file = File::new(&[42; BOOT_SEED_BYTES]);
            file.writable = writable;
            file.flush_ok = false;
            let mut seed = [99; BOOT_SEED_BYTES];
            assert!(!consume(Some(&mut file), &mut seed));
            assert_eq!(seed, [0; BOOT_SEED_BYTES]);
        }
    }
}
