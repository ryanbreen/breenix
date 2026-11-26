"""Drive the firmware suite install with file and block I/O callbacks."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
EFI = ROOT / 'scripts/metal/efi-stub/src'

MODEL = r'''
use core::{ffi::c_void, ptr, slice};
use uefi::{ok, BlockIo, BlockIoMedia, File, Status, DEVICE_ERROR, LOAD_ERROR, SUCCESS, UNSUPPORTED};

const SECTOR: usize = 512;
struct BootServices;

struct Buffer<'a> {
    data: *mut u8,
    _memory: Box<[u8]>,
    _boot: &'a BootServices,
}

impl<'a> Buffer<'a> {
    unsafe fn new(boot: &'a BootServices, size: usize, _: usize) -> Result<Self, Status> {
        let mut memory = vec![0; size].into_boxed_slice();
        Ok(Self { data: memory.as_mut_ptr(), _memory: memory, _boot: boot })
    }
}

#[repr(C)]
struct Source {
    file: File,
    bytes: Vec<u8>,
    position: usize,
}

unsafe extern "efiapi" fn source_read(file: *mut File, count: *mut usize, out: *mut c_void) -> Status {
    let source = &mut *file.cast::<Source>();
    *count = (*count).min(source.bytes.len() - source.position);
    ptr::copy_nonoverlapping(source.bytes.as_ptr().add(source.position), out.cast(), *count);
    source.position += *count;
    SUCCESS
}

unsafe extern "efiapi" fn seek(file: *mut File, offset: u64) -> Status {
    let source = &mut *file.cast::<Source>();
    if offset > source.bytes.len() as u64 {
        return DEVICE_ERROR;
    }
    source.position = offset as usize;
    SUCCESS
}

#[repr(C)]
struct Device {
    io: BlockIo,
    bytes: Vec<u8>,
    dirty: bool,
    flushes: usize,
    next_verified: usize,
    corrupt_at: Option<usize>,
}

unsafe extern "efiapi" fn write_blocks(
    io: *mut BlockIo, _: u32, lba: u64, len: usize, input: *const c_void,
) -> Status {
    let dev = &mut *io.cast::<Device>();
    let offset = lba as usize * SECTOR;
    let bytes = slice::from_raw_parts(input.cast::<u8>(), len);
    if offset == 0 && bytes.iter().any(|&byte| byte != 0) && dev.next_verified != dev.bytes.len() {
        return DEVICE_ERROR;
    }
    dev.bytes[offset..offset + len].copy_from_slice(bytes);
    dev.dirty = true;
    SUCCESS
}

unsafe extern "efiapi" fn flush(io: *mut BlockIo) -> Status {
    let dev = &mut *io.cast::<Device>();
    dev.dirty = false;
    dev.flushes += 1;
    SUCCESS
}

unsafe extern "efiapi" fn read_blocks(
    io: *mut BlockIo, _: u32, lba: u64, len: usize, out: *mut c_void,
) -> Status {
    let dev = &mut *io.cast::<Device>();
    let offset = lba as usize * SECTOR;
    if dev.dirty || (offset != 0 && dev.next_verified != offset) {
        return DEVICE_ERROR;
    }
    let bytes = slice::from_raw_parts_mut(out.cast::<u8>(), len);
    bytes.copy_from_slice(&dev.bytes[offset..offset + len]);
    if offset != 0 {
        dev.next_verified += len;
    }
    if dev.corrupt_at == Some(offset) {
        bytes[len - 1] ^= 1;
    }
    SUCCESS
}

unsafe extern "efiapi" fn unsupported<A>(_: A) -> Status { UNSUPPORTED }
unsafe extern "efiapi" fn unsupported2<A, B>(_: A, _: B) -> Status { UNSUPPORTED }
unsafe extern "efiapi" fn unsupported3<A, B, C>(_: A, _: B, _: C) -> Status { UNSUPPORTED }
unsafe extern "efiapi" fn unsupported4<A, B, C, D>(_: A, _: B, _: C, _: D) -> Status {
    UNSUPPORTED
}
unsafe extern "efiapi" fn unsupported5<A, B, C, D, E>(
    _: A, _: B, _: C, _: D, _: E,
) -> Status { UNSUPPORTED }

#[test]
fn stage_flushes_once_for_data_and_validates_before_publishing() {
    let size = SECTOR + 2 * CHUNK + SECTOR;
    let original: Vec<_> = (0..size).map(|index| (index % 251 + 1) as u8).collect();
    let record = &original[..SECTOR];
    let media = BlockIoMedia {
        media_id: 1, removable_media: 0, media_present: 1, logical_partition: 1,
        read_only: 0, write_caching: 1, block_size: SECTOR as u32, io_align: 1,
        last_block: (size / SECTOR - 1) as u64,
    };
    for corrupt_at in [Some(SECTOR + 2 * CHUNK), None] {
        let mut source = Source {
            file: File {
                revision: 1, open: unsupported5, close: unsupported, delete: unsupported,
                read: source_read, write: unsupported3, get_position: 0, set_position: seek,
                get_info: unsupported4, set_info: unsupported4, flush: unsupported,
            },
            bytes: original.clone(),
            position: 0,
        };
        let mut dev = Device {
            io: BlockIo {
                revision: 1, media: &media, reset: unsupported2, read_blocks, write_blocks,
                flush_blocks: flush,
            },
            bytes: vec![0xff; size], dirty: false, flushes: 0, next_verified: SECTOR, corrupt_at,
        };
        // SAFETY: All firmware callbacks are backed by the live fixture structs.
        let result = unsafe {
            let buffer = Buffer::new(&BootServices, CHUNK, 1).unwrap();
            install(&BootServices, &mut dev.io, &mut source.file, size, record, &buffer)
        };
        if corrupt_at.is_some() {
            assert_eq!(result, Err(LOAD_ERROR));
            assert_eq!(&dev.bytes[..SECTOR], &[0; SECTOR]);
            continue;
        }
        assert_eq!(result, Ok(()));
        assert_eq!(dev.bytes, original);
        assert_eq!(dev.next_verified, size);
        assert!(!dev.dirty);
        assert_eq!(dev.flushes, 3, "invalidate, data, publish");
    }
}
'''


class MetalSuiteStageTest(unittest.TestCase):
    def test_firmware_batches_data_flush_without_losing_readback(self):
        source = (EFI / 'suite_disk.rs').read_text()
        chunk = next(line for line in source.splitlines() if line.startswith('pub(super) const CHUNK:'))
        start = source.index('unsafe fn install(')
        end = source.index('\n}\n', start) + 3
        install = source[start:end]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'stage.rs'
            test = Path(directory) / 'stage'
            path.write_text(
                f'#[allow(dead_code)]\n#[path = "{EFI / "uefi.rs"}"]\nmod uefi;\n'
                + chunk.removeprefix('pub(super) ') + '\n' + install + MODEL
            )
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run([*priority, 'rustc', '--edition=2021', '-Dwarnings', '--test',
                            path, '-o', test], check=True, timeout=30)
            subprocess.run([test], check=True, timeout=10)


if __name__ == '__main__':
    unittest.main()
