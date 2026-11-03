"""Run the ARM capture entry point with fw_cfg forbidden off QEMU."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from screen_evidence_test import function

ROOT = Path(__file__).resolve().parents[1]


class CapturePlatform(unittest.TestCase):
    def test_platform_is_checked_before_fw_cfg(self):
        endpoint = function((ROOT / 'kernel/src/fs/devfs/screen_capture.rs').read_text(),
                            'pub fn write(request:')
        fixture = r'''
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
static QEMU: AtomicBool = AtomicBool::new(false);
static CAPTURE: AtomicBool = AtomicBool::new(true);
static READS: AtomicUsize = AtomicUsize::new(0);
mod platform_config { pub fn is_qemu() -> bool { crate::QEMU.load(crate::Ordering::Relaxed) } }
mod drivers {
    pub mod fw_cfg { pub fn read_string(_: &str) -> Option<String> {
        assert!(crate::platform_config::is_qemu(), "accessed absent fw_cfg window");
        crate::READS.fetch_add(1, crate::Ordering::Relaxed);
        Some(if crate::CAPTURE.load(crate::Ordering::Relaxed) { "on" } else { "off" }.into())
    } }
    pub mod virtio { pub mod block_mmio { pub const MAX_BLOCK_DEVICES: usize = 1; } }
}
mod spin {
    pub struct Mutex<T>(std::sync::Mutex<T>);
    impl<T> Mutex<T> {
        pub const fn new(value: T) -> Self { Self(std::sync::Mutex::new(value)) }
        pub fn try_lock(&self) -> Option<std::sync::MutexGuard<'_, T>> { self.0.try_lock().ok() }
    }
}
mod block {
    pub trait BlockDevice {
        fn read_block(&self, _: u64, _: &mut [u8]) -> Result<(), ()> { panic!("disk read") }
        fn write_block(&self, _: u64, _: &[u8]) -> Result<(), ()> { panic!("disk write") }
        fn flush(&self) -> Result<(), ()> { panic!("disk flush") }
        fn num_blocks(&self) -> u64 { panic!("disk size") }
    }
    pub mod virtio {
        pub struct VirtioBlockWrapper;
        impl VirtioBlockWrapper { pub fn new(_: usize) -> Option<Self> { None } }
        impl super::BlockDevice for VirtioBlockWrapper {}
    }
}
mod syscall { pub mod userptr {
    pub fn read_user_bytes(_: *mut u8, _: u64, _: usize) -> Result<(), ()> { panic!("user read") }
} }
mod screen_capture {
    pub const DISK_MAGIC: &[u8; 8] = b"SCRDISK1";
    pub const MAX_RECORD: usize = 16 * 1024 * 1024;
    pub struct CaptureLog;
    impl CaptureLog {
        pub const fn new() -> Self { Self }
        pub fn append_stream(&mut self, _: usize, _: u64,
            _: impl FnMut(usize, &mut [u8]) -> Result<(), &'static str>,
            _: impl FnMut(u64, &[u8]) -> Result<(), &'static str>,
            _: impl FnMut() -> Result<(), &'static str>) -> Result<bool, &'static str> {
            panic!("capture I/O")
        }
    }
}
ENDPOINT
#[test] fn non_qemu_never_reads_fw_cfg() {
    for enabled in [false, true] {
        CAPTURE.store(enabled, Ordering::Relaxed);
        assert_eq!(write(&[]), Err(19));
        assert_eq!(READS.load(Ordering::Relaxed), 0);
    }
    QEMU.store(true, Ordering::Relaxed);
    CAPTURE.store(false, Ordering::Relaxed);
    assert_eq!(write(&[]), Err(19));
    CAPTURE.store(true, Ordering::Relaxed);
    assert_eq!(write(&[]), Err(22));
    assert_eq!(READS.load(Ordering::Relaxed), 2);
}
'''.replace('ENDPOINT', endpoint)
        with tempfile.TemporaryDirectory() as directory:
            p = Path(directory)
            (p / 'test.rs').write_text(fixture)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                                      str(p / 'test.rs'), '-o', str(p / 'test')], check=True)
            subprocess.run([str(p / 'test')], check=True)


if __name__ == '__main__':
    unittest.main()
