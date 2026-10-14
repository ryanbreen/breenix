#!/usr/bin/env python3
"""Run production exec-image parsing with explicit filesystem/user-copy shims."""
from pathlib import Path
import os
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
source = (ROOT / "kernel/src/syscall/exec.rs").read_text()
arguments = source[source.index("pub(crate) struct Arguments"):source.index("pub(crate) fn copy_string")]
images = source[source.index("fn convenience_path"):source.index("/// Map enough initial stack")]
harness = r'''
extern crate alloc;
use alloc::{string::String, vec::Vec};
use core::ops::Range;
const DEFAULT_STACK_LIMIT: u64 = 1024 * 1024;
const ARG_MAX: usize = DEFAULT_STACK_LIMIT as usize / 4;
mod errno {
    pub const E2BIG: i32 = 7;
    pub const EACCES: i32 = 13;
    pub const EFAULT: i32 = 14;
    pub const EISDIR: i32 = 21;
    pub const ELOOP: i32 = 40;
    pub const ENOEXEC: i32 = 8;
    pub const ENOMEM: i32 = 12;
}
use errno::*;
mod userptr {
    pub fn copy_from_user<T: Copy>(ptr: *const T) -> Result<T, u64> {
        if ptr.is_null() { return Err(14); }
        Ok(unsafe { ptr.read_unaligned() })
    }
    pub fn read_user_bytes(dst: *mut u8, src: u64, len: usize) -> Result<(), u64> {
        if src == 0 { return Err(14); }
        unsafe { core::ptr::copy_nonoverlapping(src as *const u8, dst, len); }
        Ok(())
    }
}
mod memory { pub mod layout { pub const USER_STACK_REGION_END: u64 = 0x800000000000; } }
mod fs { pub mod namei {
    pub fn current_working_dir() {}
    pub fn working_dir_path_bytes(_: &()) -> Result<Vec<u8>, u64> { Ok(b"/tmp/cwd".to_vec()) }
} }
fn elf() -> Vec<u8> {
    let mut b = vec![0; 120];
    b[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    b[16..18].copy_from_slice(&2u16.to_le_bytes());
    b[18..20].copy_from_slice(&183u16.to_le_bytes());
    b[52..54].copy_from_slice(&64u16.to_le_bytes());
    b[54..56].copy_from_slice(&56u16.to_le_bytes());
    b[56..58].copy_from_slice(&1u16.to_le_bytes());
    b[32..40].copy_from_slice(&64u64.to_le_bytes());
    b[24..32].copy_from_slice(&0x10000u64.to_le_bytes());
    b[64..68].copy_from_slice(&1u32.to_le_bytes());
    b[68..72].copy_from_slice(&1u32.to_le_bytes());
    b[80..88].copy_from_slice(&0x10000u64.to_le_bytes());
    b[96..104].copy_from_slice(&120u64.to_le_bytes());
    b[104..112].copy_from_slice(&120u64.to_le_bytes());
    b
}
#[test]
fn relative_image_path_is_absolute() {
    let arg = b"argument\0";
    let vector = [arg.as_ptr() as u64, 0];
    let copied = Arguments::copy_from_user(b"/bin/child", vector.as_ptr() as u64, 0).unwrap();
    assert_eq!(copied.slices().unwrap().0, [arg.as_slice()]);
    for (input, expected) in [(b"./child".as_slice(), b"/tmp/cwd/./child".as_slice()),
                              (b"tools/child".as_slice(), b"/tmp/cwd/tools/child".as_slice()),
                              (b"child".as_slice(), b"/bin/child".as_slice())] {
        let mut args = Arguments::copy_from_user(input, 0, 0).unwrap();
        let (_, _, path) = read_image(input, &mut args, |path| {
            assert_eq!(path, expected);
            Ok((elf(), ()))
        }).unwrap();
        assert_eq!(path.as_bytes(), expected);
    }
}
#[test]
fn non_utf8_image_and_script_arguments_are_preserved() {
    let input = b"/tmp/child\xff";
    let mut args = Arguments::copy_from_user(input, 0, 0).unwrap();
    let mut reads = 0;
    let (_, _, image) = read_image(input, &mut args, |path| {
        reads += 1;
        if reads == 1 {
            assert_eq!(path, input);
            Ok((b"#!/bin/interpreter --flag\xfe\n".to_vec(), ()))
        } else {
            assert_eq!(path, b"/bin/interpreter");
            Ok((elf(), ()))
        }
    }).unwrap();
    assert_eq!(image, "/bin/interpreter");
    let (argv, envp) = args.slices().unwrap();
    assert_eq!(argv, [b"/bin/interpreter\0".as_slice(), b"--flag\xfe\0".as_slice(), b"/tmp/child\xff\0".as_slice()]);
    assert!(envp.is_empty());
}
#[test]
fn missing_non_utf8_file_preserves_enoent() {
    let mut args = Arguments::copy_from_user(b"/tmp/missing\xff", 0, 0).unwrap();
    assert_eq!(read_image(b"/tmp/missing\xff", &mut args, |_| Err::<(Vec<u8>, ()), _>(2)).unwrap_err(), 2);
}
#[test]
fn interpreter_cycle_and_invalid_elf_are_rejected() {
    let mut args = Arguments::copy_from_user(b"/loop", 0, 0).unwrap();
    assert_eq!(read_image(b"/loop", &mut args, |_| Ok((b"#!/loop\n".to_vec(), ()))).unwrap_err(), 40);
    assert_eq!(read_image(b"/bad", &mut args, |_| Ok((vec![0; 64], ()))).unwrap_err(), 8);
}
'''
# The production module's super is the harness root in this isolated compilation.
production = (arguments + images).replace("super::userptr::", "userptr::").replace("super::errno::", "errno::")
with tempfile.TemporaryDirectory(prefix="breenix-exec-image-") as tmp:
    test = Path(tmp) / "tests.rs"
    test.write_text(harness + production)
    binary = Path(tmp) / "tests"
    env = dict(os.environ, CARGO_BUILD_JOBS="6")
    subprocess.run(["rustc", "--edition=2021", "--test", str(test), "-o", str(binary)], cwd=ROOT, env=env, check=True)
    subprocess.run([str(binary)], check=True)
