"""Run the affected production Rust with host stand-ins for kernel services."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]
SYSCALL = ROOT / "kernel/src/syscall"
BUILD = ROOT / "target/rustiness-host"
TASKPOLICY = ["taskpolicy", "-b"] if sys.platform == "darwin" else []

# All addresses refer to ordinary host allocations. The layout shim labels one
# as kernel memory, and the uaccess shim reports bytes left at a simulated
# mapping boundary, just as the architecture-specific copy routine would.
UACCESS = r"""
#[derive(Debug, PartialEq)]
pub enum SyscallResult { Ok(u64), Err(u64) }

mod memory {
    pub mod layout {
        use std::cell::Cell;

        thread_local! {
            static MAPPED: Cell<(u64, usize)> = const { Cell::new((0, 0)) };
            static KERNEL: Cell<u64> = const { Cell::new(0) };
        }

        pub fn set_regions(mapped: u64, len: usize, kernel: u64) {
            MAPPED.set((mapped, len));
            KERNEL.set(kernel);
        }

        pub fn is_valid_user_range(addr: u64, len: usize) -> bool {
            addr != 0 && addr != KERNEL.get() && addr.checked_add(len as u64).is_some()
        }

        pub fn mapped_prefix(addr: u64, len: usize) -> usize {
            let (base, size) = MAPPED.get();
            addr.checked_sub(base).map_or(0, |offset| size.saturating_sub(offset as usize)).min(len)
        }
    }
}
"""

WRITE_COPY = (
    "pub fn validate_user_ptr_read<",
    "pub fn validate_user_ptr_write<",
    "pub fn write_user_bytes(",
    "pub fn write_user_prefix(",
    "pub fn copy_to_user<",
)


def checked_copy(functions):
    source = (SYSCALL / "userptr.rs").read_text()
    return "\n".join(item(source, marker) for marker in functions)


def run_test(fixture, name):
    BUILD.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=BUILD) as tmp:
        path = Path(tmp)
        (path / "regression.rs").write_text(fixture)
        subprocess.run(
            TASKPOLICY + ["rustc", "--edition=2021", "--test", "-Dwarnings",
                          "regression.rs", "-o", "regression"],
            cwd=path, check=True, timeout=25,
        )
        subprocess.run([str(path / "regression"), "--exact", name],
                       cwd=path, check=True, timeout=25)


class RustinessRegressions(unittest.TestCase):
    def test_sigpending(self):
        source = (SYSCALL / "signal.rs").read_text()
        handler = item(source, "pub fn sys_sigpending(")
        # The pending mask is a scheduler snapshot. Supply it directly and
        # execute the handler's production path from snapshot to syscall result.
        tail = handler[handler.index("    // Write pending signal set to userspace"):]
        fixture = UACCESS + r"""
extern crate self as log;
#[macro_export]
macro_rules! debug { ($($arg:tt)*) => {} }

pub mod userptr {
    use crate::memory::layout::mapped_prefix;

    unsafe fn write(dst: *mut u8, src: *const u8, len: usize) -> usize {
        let copied = mapped_prefix(dst as u64, len);
        if copied != 0 {
            unsafe { std::ptr::copy_nonoverlapping(src, dst, copied); }
        }
        len - copied
    }

    #[cfg(target_arch = "x86_64")]
    use write as breenix_uaccess_copy;
    #[cfg(target_arch = "aarch64")]
    use write as breenix_uaccess_write;

    // CHECKED_COPY
}

#[allow(unused_imports)]
use userptr::copy_to_user;

fn return_pending(set: u64, pending: u64) -> SyscallResult {
    // SIGNAL_TAIL

#[test]
fn sigpending_output_respects_mapping() {
    let mut valid = 0u64;
    let mut unmapped = 0u64;
    let mut kernel = 0u64;
    let valid_addr = &raw mut valid as u64;
    let unmapped_addr = &raw mut unmapped as u64;
    let kernel_addr = &raw mut kernel as u64;
    memory::layout::set_regions(valid_addr, size_of::<u64>(), kernel_addr);

    assert_eq!(return_pending(valid_addr, 0x400), SyscallResult::Ok(0));
    assert_eq!(valid, 0x400);
    assert_eq!(return_pending(unmapped_addr, 0x400), SyscallResult::Err(14));
    assert_eq!(return_pending(kernel_addr, 0x400), SyscallResult::Err(14));
}
"""
        fixture = fixture.replace("// CHECKED_COPY", checked_copy(WRITE_COPY))
        fixture = fixture.replace("// SIGNAL_TAIL", tail)
        run_test(fixture, "sigpending_output_respects_mapping")


if __name__ == "__main__":
    unittest.main()
