"""Execute the x86 framebuffer dispatch and fbdraw's early errno decisions.

The harness compiles source slices, with user-copy and legacy handlers replaced
at their boundaries. It does not exercise rendering or claim boot evidence.
"""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class RetiredFramebuffer(unittest.TestCase):
    def test_x86_errno_contract(self):
        handler = (ROOT / 'kernel/src/syscall/handler.rs').read_text()
        arms = handler.split('        // Graphics syscalls\n', 1)[1].split(
            '        Some(SyscallNumber::GetMousePos)', 1)[0]
        numbers = (ROOT / 'kernel/src/syscall/mod.rs').read_text().split(
            '            410 =>', 1)[1].split('            413 =>', 1)[0]
        graphics = (ROOT / 'kernel/src/syscall/graphics.rs').read_text()
        draw = item(graphics, 'pub fn sys_fbdraw(')
        # Stop before process/display access: the assertions exercise only the
        # copied command and unsupported-op decisions above that boundary.
        draw = draw.split('    let (fb_mmap, caller_owns_display, is_root)', 1)[0]
        draw += '    get_current_thread_id(); SyscallResult::Ok(99)\n}\n'
        fixture = r'''
pub mod syscall {
    #[derive(Debug, PartialEq)]
    pub enum SyscallResult { Ok(u64), Err(u64) }
    pub enum ErrorCode { InvalidArgument = 22, Fault = 14, NoSys = 38 }
    pub enum SyscallNumber { FbInfo, FbDraw, FbMmap }
    pub mod memory_common { pub fn get_current_thread_id() {} }
    pub mod userptr {
        pub fn copy_from_user<T: Copy>(ptr: *const T) -> Result<T, ()> {
            if ptr.is_null() { return Err(()); }
            Ok(unsafe { ptr.read() })
        }
    }
    pub mod graphics {
        use super::SyscallResult;
        DRAW_CMD
        DRAW
        // Distinct successes expose a dispatch that still calls a legacy path.
        pub fn sys_fbinfo(_: u64) -> SyscallResult { SyscallResult::Ok(410) }
        pub fn sys_fbmmap(_: u64) -> SyscallResult { SyscallResult::Ok(412) }
    }
    pub mod handler {
        use super::{SyscallNumber, SyscallResult};
        pub fn dispatch(number: u64, arg: u64) -> SyscallResult {
            let args = (arg,);
            // The repaired arms need no argument, but the legacy arms do.
            let _ = args;
            let call = match number {
                410 => NUMBERS
                _ => None,
            };
            match call { ARMS _ => SyscallResult::Err(38) }
        }
    }
}
use syscall::{graphics::FbDrawCmd, SyscallResult};
#[cfg(feature = "testing")]
use syscall::graphics::sys_fbdraw;
fn command(op: u32) -> FbDrawCmd {
    FbDrawCmd { op, p1: 0, p2: 0, p3: 0, p4: 0, color: 0 }
}
#[cfg(not(feature = "testing"))]
#[test]
fn retired_syscalls_return_enosys_before_accessing_arguments() {
    for number in 410..=412 {
        for op in [0, 27, 28, 29, 30] {
            let cmd = command(op);
            for arg in [0, &cmd as *const _ as u64] {
                assert_eq!(syscall::handler::dispatch(number, arg), SyscallResult::Err(38),
                           "retired syscall {number}, op {op}");
            }
        }
    }
}
#[cfg(feature = "testing")]
#[test]
fn testing_dispatch_keeps_framebuffer_handlers() {
    assert_eq!(syscall::handler::dispatch(410, 0), SyscallResult::Ok(410));
    assert_eq!(syscall::handler::dispatch(411, 0), SyscallResult::Err(14));
    assert_eq!(syscall::handler::dispatch(412, 0), SyscallResult::Ok(412));
}
#[cfg(feature = "testing")]
#[test]
fn testing_fbdraw_rejects_unsupported_ops() {
    for op in 27..=30 {
        let cmd = command(op);
        assert_eq!(sys_fbdraw(&cmd as *const _ as u64), SyscallResult::Err(22),
                   "wait-stress op {op}");
    }
    assert_eq!(sys_fbdraw(0), SyscallResult::Err(14));
    for op in [0, 26, 31, 33, u32::MAX] {
        let cmd = command(op);
        assert_eq!(sys_fbdraw(&cmd as *const _ as u64), SyscallResult::Err(22));
    }
}
'''
        fixture = fixture.replace('DRAW_CMD', item(graphics, 'pub struct FbDrawCmd'))
        # The source struct's derive/repr attributes precede the extracted item.
        fixture = fixture.replace('pub struct FbDrawCmd', '#[derive(Clone, Copy)]\n        #[repr(C)]\n        pub struct FbDrawCmd')
        fixture = fixture.replace('DRAW', draw).replace(
            'NUMBERS', numbers.replace('Self::', 'SyscallNumber::')).replace('ARMS', arms)
        with tempfile.TemporaryDirectory(prefix='x86-retired-framebuffer-') as tmp:
            source = Path(tmp) / 'check.rs'
            source.write_text(fixture)
            for testing in [False, True]:
                with self.subTest(testing=testing):
                    binary = Path(tmp) / ('testing' if testing else 'production')
                    target = 'x86_64-apple-darwin' if sys.platform == 'darwin' else 'x86_64-unknown-linux-gnu'
                    args = ['rustc', '--edition=2021', '--target', target,
                            '--test', '-Dwarnings', str(source), '-o', str(binary)]
                    if testing:
                        args += ['--cfg', 'feature="testing"']
                    if sys.platform == 'darwin':
                        args = ['taskpolicy', '-b', *args]
                    build = subprocess.run(args, capture_output=True, text=True)
                    self.assertEqual(build.returncode, 0, build.stdout + build.stderr)
                    run = subprocess.run([str(binary), '--nocapture'], capture_output=True, text=True)
                    self.assertEqual(run.returncode, 0, run.stdout + run.stderr)


if __name__ == '__main__':
    unittest.main()
