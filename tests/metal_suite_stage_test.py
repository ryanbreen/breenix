"""Drive the firmware suite install with file and block I/O callbacks."""
import json
from pathlib import Path
import runpy
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import metal_tools_test as existing

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
            install(&BootServices, &mut dev.io, &mut source.file, StageGeometry::dense(size), record, &buffer, |_| Ok(()))
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
#[test]
fn sparse_body_reads_only_declared_blocks_and_bounds_the_last_extent() {
    let dense = 2 * SECTOR;
    let scratch = 3 * 4096;
    let disk = dense + scratch;
    let mut bytes = vec![0x11; dense];
    for offset in [0, 2 * 4096] {
        bytes.extend_from_slice(&(offset as u64).to_le_bytes());
        bytes.extend_from_slice(&4096u64.to_le_bytes());
        bytes.extend_from_slice(&[0x22; 4096]);
    }
    let geometry = StageGeometry { dense, disk, wire: bytes.len() };
    let mut source = Source {
        file: File {
            revision: 1, open: unsupported5, close: unsupported, delete: unsupported,
            read: source_read, write: unsupported3, get_position: 0, set_position: seek,
            get_info: unsupported4, set_info: unsupported4, flush: unsupported,
        },
        bytes,
        position: SECTOR,
    };
    let mut ranges = Vec::new();
    // SAFETY: The file and buffer use the live host fixtures above.
    unsafe {
        let buffer = Buffer::new(&BootServices, CHUNK, 1).unwrap();
        walk_body(&mut source.file, geometry, &buffer, |lba, count, consumed| {
            ranges.push((lba, count, consumed));
            Ok(())
        }).unwrap();
        assert_eq!(ranges, [(1, SECTOR, dense), (2, 4096, dense + 16 + 4096), (18, 4096, geometry.wire)]);
        source.position = SECTOR;
        // Move the final extent beyond the recorded slice.
        let last_header = dense + 16 + 4096;
        source.bytes[last_header..last_header + 8].copy_from_slice(&(scratch as u64).to_le_bytes());
        ranges.clear();
        let result = walk_body(&mut source.file, geometry, &buffer, |lba, count, consumed| {
            ranges.push((lba, count, consumed));
            Ok(())
        });
        assert_eq!(result, Err(LOAD_ERROR));
        assert_eq!(ranges, [(1, SECTOR, dense), (2, 4096, dense + 16 + 4096)]);
    }
}

'''


class MetalSuiteStageTest(unittest.TestCase):
    def test_toolchain_and_self_host_stay_out_of_aggregate_selection(self):
        resolve = runpy.run_path(str(ROOT / 'scripts/suite-sequence.py'))['resolve']
        names, _ = resolve('all', 'x86-metal', has_3d=True)
        self.assertNotIn('self-host', names)
        self.assertNotIn('toolchain', names)
        selected = 'memory,processes,self-host'
        self.assertEqual(resolve(selected, 'x86-metal')[0], selected.split(','))

    def test_self_host_carries_only_packed_scratch_extents(self):
        disk = existing.boot.metal_disk
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            image = root / 'ext2.img'
            root_bytes = 2048
            image.write_bytes(bytes(1080) + b'\x53\xef' + bytes(root_bytes - 1082))
            scratch_bytes = (64 << 20) + 4096
            packed = root / 'self-host-scratch.dat'
            packed.write_bytes(struct.pack('<QQ', scratch_bytes - 4096, 4096) + bytes(4096))
            build = {
                'ext2': existing.boot.metal_prebuilt.facts(image),
                'selfHost': {'scratchBytes': scratch_bytes,
                             'disk': existing.boot.metal_prebuilt.facts(packed)},
            }
            partition = '00000000-0000-0000-0000-000000000001'
            with patch.object(disk.metal_persist, 'test_root', return_value=None), \
                 patch.object(disk, 'test_partition_bytes', return_value=1 << 30), \
                 patch.object(disk.metal_root, 'console_identity', return_value={}), \
                 patch.object(disk.metal_root, 'stage_root', return_value={}), \
                 patch.object(disk.m, 'power_token', return_value='test-token'):
                facts, files = disk.stage(root, root, '20261126T000000Z-metal', 'a' * 40, {}, build,
                                          mode='suite', suite='memory,processes,self-host',
                                          the_layout={'partitions': {'test': {'guid': partition}}})
            wire_bytes = root_bytes + packed.stat().st_size
            self.assertEqual(facts['stagedBytes'], wire_bytes)
            self.assertEqual(facts['fixtureSlices'],
                             [{'role': 'scratch', 'offset': root_bytes, 'length': scratch_bytes}])
            self.assertEqual(files['suite-root.img'].read_bytes()[root_bytes:], packed.read_bytes())
            self.assertTrue(files['suite-disk.cfg'].read_bytes().startswith(disk.MAGIC_3))

    def test_self_host_fixture_matches_mac_git_trees_and_reserves_headroom(self):
        script = ROOT / 'scripts/rust-host/selfhost-fixture.py'
        self.assertTrue(script.is_file(), 'the self-host scratch fixture producer is missing')
        fixture = runpy.run_path(str(script))['main']
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            repo = root / 'repo'
            repo.mkdir()
            for name in ('Cargo.toml', 'Cargo.lock', 'bobj/Cargo.toml', 'bobj/src/lib.rs'):
                path = repo / 'toolchain' / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(name)

            def git(*args):
                return subprocess.check_output(['git', '-C', repo, *args], text=True).strip()

            git('init', '-q')
            git('add', 'toolchain')
            git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                'commit', '-qm', 'Record workspace')
            receipt = root / 'mac.json'
            receipt.write_text(json.dumps({'workspace': {
                name: git('rev-parse', f'HEAD:toolchain/{name}')
                for name in ('Cargo.toml', 'Cargo.lock', 'bobj')
            }}))
            formatter = root / 'mkfs'
            formatter.write_text("""#!/bin/sh
size=$3
python3 -c 'import sys; open(sys.argv[1], "wb").truncate(int(sys.argv[2]))' "$6" "$size"
printf '0 1\n%s 1\n' "$((size / 4096 - 1))"
""")
            formatter.chmod(0o755)
            disk = root / 'self-host-scratch.dat'
            with patch.dict(fixture.__globals__, ROOT=repo, MAC_REFERENCE=receipt):
                fixture(root / 'tree', formatter, disk)
                record = json.loads((root / 'self-host-build.json').read_text())
                self.assertEqual((root / 'tree/breenix-self-host').read_bytes(), b'e25-self-host\n')
                payload = sum(path.stat().st_size for path in (root / 'tree').rglob('*') if path.is_file())
                self.assertEqual(record['payload_bytes'], payload)
                self.assertEqual(record['scratchBytes'], ((payload + (64 << 20) + 4095) // 4096) * 4096)
                self.assertEqual(disk.stat().st_size, 2 * (16 + 4096))
                (repo / 'toolchain/bobj/src/lib.rs').write_text('changed after the Mac replay')
                git('add', 'toolchain')
                git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                    'commit', '-qm', 'Change workspace')
                with self.assertRaisesRegex(ValueError, 'toolchain/bobj.*Mac twin'):
                    fixture(root / 'changed', formatter, root / 'changed.dat')
                self.assertFalse((root / 'changed.dat').exists())

    def test_firmware_batches_data_flush_without_losing_readback(self):
        source = (EFI / 'suite_disk.rs').read_text()
        chunk = next(line for line in source.splitlines() if line.startswith('pub(super) const CHUNK:'))
        start = source.index('#[derive(Clone, Copy)]\nstruct StageGeometry')
        end = source.index('/// The HTTP binding', start)
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
