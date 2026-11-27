//! Host model of ELF interpreter handoff (gABI ch. 5 and AMD64 psABI §3.4).
#![cfg(test)]
extern crate alloc;

#[path = "../kernel/src/exec_elf.rs"]
mod exec_elf;
#[path = "../kernel/src/syscall/errno.rs"]
pub mod syscall_errno;
mod syscall {
    pub use crate::syscall_errno as errno;
}
mod fs {
    pub mod errno {
        #[derive(Debug, PartialEq, Eq)]
        pub struct Errno(pub i32);
    }
}

use exec_elf::Headers;
use fs::errno::Errno;
use syscall::errno::{ENOENT, ENOEXEC};

const PROGRAM_BASE: u64 = 0x4000_0000;
const PROGRAM_ENTRY: u64 = PROGRAM_BASE + 0x300;
const INTERP_BASE: u64 = 0x1000;
const INTERP_ENTRY: u64 = INTERP_BASE + 0x400;
const PAGE: u64 = 4096;
const RANDOM: u64 = 0x7fff_feff_fff0;
const PATH: &[u8] = b"/lib/ld-breenix.so\0";
const IDS: [u64; 5] = [1000, 1001, 2000, 2001, 1];
const EM_AARCH64: u16 = 183;

fn put(bytes: &mut [u8], offset: usize, value: &[u8]) {
    bytes[offset..offset + value.len()].copy_from_slice(value);
}

fn elf(kind: u16, base: u64, entry: u64, path: Option<&[u8]>) -> Vec<u8> {
    let mut bytes = vec![0; PAGE as usize];
    let phnum = if path.is_some() { 2u16 } else { 1 };
    put(&mut bytes, 0, b"\x7fELF\x02\x01\x01");
    put(&mut bytes, 16, &kind.to_le_bytes());
    put(&mut bytes, 18, &62u16.to_le_bytes());
    put(&mut bytes, 20, &1u32.to_le_bytes());
    put(&mut bytes, 24, &entry.to_le_bytes());
    put(&mut bytes, 32, &64u64.to_le_bytes());
    put(&mut bytes, 52, &64u16.to_le_bytes());
    put(&mut bytes, 54, &56u16.to_le_bytes());
    put(&mut bytes, 56, &phnum.to_le_bytes());
    if let Some(path) = path {
        put(&mut bytes, 64, &3u32.to_le_bytes()); // PT_INTERP, before PT_LOAD
        put(&mut bytes, 64 + 8, &0x200u64.to_le_bytes());
        put(&mut bytes, 64 + 32, &(path.len() as u64).to_le_bytes());
        put(&mut bytes, 0x200, path);
    }
    let load = 64 + usize::from(phnum - 1) * 56;
    put(&mut bytes, load, &1u32.to_le_bytes()); // PT_LOAD
    put(&mut bytes, load + 4, &5u32.to_le_bytes()); // PF_R | PF_X
    put(&mut bytes, load + 16, &base.to_le_bytes());
    put(&mut bytes, load + 32, &PAGE.to_le_bytes());
    put(&mut bytes, load + 40, &PAGE.to_le_bytes());
    put(&mut bytes, load + 48, &PAGE.to_le_bytes());
    bytes
}

fn read(bytes: &[u8], machine: u16) -> Result<Headers, Errno> {
    Headers::read(
        &bytes[..64],
        bytes.len() as u64,
        machine,
        1 << 47,
        &mut |offset, dst| {
            let offset = offset as usize;
            dst.copy_from_slice(&bytes[offset..offset + dst.len()]);
            Ok(())
        },
    )
}

#[test]
fn interp_handoff_auxv() {
    let mut program = elf(2, PROGRAM_BASE, PROGRAM_ENTRY, Some(PATH));
    let headers = read(&program, 62).expect("read the executable with PT_INTERP");
    assert_eq!(headers.interpreter_path(), Some(&PATH[..PATH.len() - 1]));

    let interpreter = read(&elf(3, INTERP_BASE, INTERP_ENTRY, None), 62).unwrap();
    // The region starts inside the program's load, so placement must skip it.
    let region = PROGRAM_BASE..PROGRAM_BASE + 0x10000;
    let plan = headers
        .exec_plan(Some((&interpreter, region.clone())))
        .unwrap();
    let bias = plan.interpreter_bias().unwrap();
    assert_eq!(bias % PAGE, 0);
    assert!(bias + INTERP_BASE >= PROGRAM_BASE + PAGE);
    assert!(bias + INTERP_BASE + PAGE <= region.end);
    assert_eq!(plan.initial_pc(), INTERP_ENTRY + bias);
    assert_eq!(
        plan.auxv(RANDOM, IDS).collect::<Vec<_>>(),
        [
            (9, PROGRAM_ENTRY),
            (3, PROGRAM_BASE + 64),
            (5, 2),
            (4, 56),
            (6, PAGE),
            (7, bias),
            (0x6000_0000, 0),
            (25, RANDOM),
            (11, 1000),
            (12, 1001),
            (13, 2000),
            (14, 2001),
            (23, 1),
            (0, 0),
        ]
    );
    assert!(headers.exec_plan(None).is_err());

    // A pathname must end within the segment, even when the next byte is NUL.
    program[0x200 + PATH.len() - 1] = b'x';
    assert!(matches!(read(&program, 62), Err(Errno(ENOEXEC))));
    put(&mut program, 0x200, PATH);
    put(&mut program, 64 + 32, &(PAGE + 1).to_le_bytes());
    program.resize(2 * PAGE as usize, 0);
    assert!(matches!(read(&program, 62), Err(Errno(ENOEXEC))));
}

fn arm_elf(kind: u16, base: u64, entry: u64, path: Option<&[u8]>) -> Vec<u8> {
    let mut bytes = elf(kind, base, entry, path);
    put(&mut bytes, 18, &EM_AARCH64.to_le_bytes());
    bytes
}

#[test]
fn interp_handoff_auxv_aarch64() {
    let mut program = arm_elf(2, PROGRAM_BASE, PROGRAM_ENTRY, Some(PATH));
    program.resize(2 * PAGE as usize, 0);
    let headers = read(&program, EM_AARCH64).expect("read the ARM executable with PT_INTERP");
    assert_eq!(headers.interpreter_path(), Some(&PATH[..PATH.len() - 1]));

    let interpreter = arm_elf(3, INTERP_BASE, INTERP_ENTRY, None);
    // Start the placement range inside the program's load.
    let region = PROGRAM_BASE..PROGRAM_BASE + 0x10000;
    let (page, plan, regions) = unsafe {
        // The interpreter has one load. The mapper lends its owned page.
        headers.load_interpreter(
            Box::new([0; PAGE as usize]),
            region.clone(),
            |path, consume| {
                assert_eq!(path, &PATH[..PATH.len() - 1]);
                consume(interpreter.len() as u64, &mut |offset, dst| {
                    let offset = offset as usize;
                    dst.copy_from_slice(&interpreter[offset..offset + dst.len()]);
                    Ok(())
                })
            },
            |page, address, flags| {
                assert_eq!(address, PROGRAM_BASE + PAGE);
                assert_eq!(flags, 5);
                Ok(page.as_mut_ptr())
            },
        )
    }.unwrap();
    assert_eq!(&page[..], &interpreter);
    let bias = plan.interpreter_bias().unwrap();
    assert_eq!(bias % PAGE, 0);
    assert!(bias + INTERP_BASE >= PROGRAM_BASE + PAGE);
    assert!(bias + INTERP_BASE + PAGE <= region.end);
    assert_eq!(plan.initial_pc(), INTERP_ENTRY + bias);
    assert!(regions.iter().any(|r| r.start == bias + INTERP_BASE && r.end == r.start + PAGE));
    assert_eq!(
        plan.auxv(RANDOM, IDS).collect::<Vec<_>>(),
        [
            (9, PROGRAM_ENTRY),
            (3, PROGRAM_BASE + 64),
            (5, 2),
            (4, 56),
            (6, PAGE),
            (7, bias),
            (0x6000_0000, 0),
            (25, RANDOM),
            (11, 1000),
            (12, 1001),
            (13, 2000),
            (14, 2001),
            (23, 1),
            (0, 0),
        ]
    );
    assert!(headers.exec_plan(None).is_err());

    let interpreter = read(&interpreter, EM_AARCH64).unwrap();
    let pie = arm_elf(3, INTERP_BASE, INTERP_ENTRY, Some(PATH));
    let pie = read(&pie, EM_AARCH64).unwrap().position_program(region.clone()).unwrap();
    let plan = pie.exec_plan(Some((&interpreter, region))).unwrap();
    let auxv: std::collections::BTreeMap<_, _> = plan.auxv(RANDOM, IDS).collect();
    assert_eq!(auxv[&0x6000_0000], PROGRAM_BASE - INTERP_BASE);
    assert_eq!(auxv[&3], PROGRAM_BASE + 64);
    assert_eq!(auxv[&9], PROGRAM_BASE + 0x400);
    assert!(plan.initial_pc() >= PROGRAM_BASE + PAGE);
}

#[test]
fn missing_interp_fails_aarch64() {
    let program = arm_elf(2, PROGRAM_BASE, PROGRAM_ENTRY, Some(PATH));
    let headers = read(&program, EM_AARCH64).expect("read the ARM executable with PT_INTERP");
    let result = unsafe {
        // The missing file must prevent the mapper from being called.
        headers.load_interpreter(
            (),
            PROGRAM_BASE..PROGRAM_BASE + 0x10000,
            |path, _| {
                assert_eq!(path, &PATH[..PATH.len() - 1]);
                Err(Errno(ENOENT))
            },
            |_, _, _| panic!("missing interpreter reached the mapper"),
        )
    };
    assert!(matches!(result, Err(Errno(ENOENT))));
}

#[test]
fn interpreter_machine_must_match_program() {
    let arm_program = arm_elf(2, PROGRAM_BASE, PROGRAM_ENTRY, Some(PATH));
    let x86_program = elf(2, PROGRAM_BASE, PROGRAM_ENTRY, Some(PATH));
    let arm_interpreter = arm_elf(3, INTERP_BASE, INTERP_ENTRY, None);
    let x86_interpreter = elf(3, INTERP_BASE, INTERP_ENTRY, None);
    for (program, machine, interpreter) in [
        (&x86_program, 62, &arm_interpreter),
        (&arm_program, EM_AARCH64, &x86_interpreter),
    ] {
        let headers = read(program, machine).expect("read the executable with PT_INTERP");
        let result = unsafe {
            // The wrong machine must prevent the mapper from being called.
            headers.load_interpreter(
                (),
                PROGRAM_BASE..PROGRAM_BASE + 0x10000,
                |_, consume| consume(interpreter.len() as u64, &mut |offset, dst| {
                    let offset = offset as usize;
                    dst.copy_from_slice(&interpreter[offset..offset + dst.len()]);
                    Ok(())
                }),
                |_, _, _| panic!("interpreter for the wrong machine reached the mapper"),
            )
        };
        assert!(matches!(result, Err(Errno(ENOEXEC))));
    }
}

#[test]
fn static_auxv_unchanged_aarch64() {
    let program = arm_elf(2, PROGRAM_BASE, PROGRAM_ENTRY, None);
    let headers = read(&program, EM_AARCH64).unwrap();
    assert_eq!(headers.interpreter_path(), None);
    let plan = headers.exec_plan(None).unwrap();
    assert_eq!(plan.initial_pc(), PROGRAM_ENTRY);
    assert_eq!(
        plan.auxv(RANDOM, IDS).collect::<Vec<_>>(),
        [
            (9, PROGRAM_ENTRY),
            (3, PROGRAM_BASE + 64),
            (5, 1),
            (4, 56),
            (6, PAGE),
            (25, RANDOM),
            (11, 1000),
            (12, 1001),
            (13, 2000),
            (14, 2001),
            (23, 1),
            (0, 0),
        ]
    );
}

#[test]
fn pie_handoff_supplies_bias_without_pt_phdr() {
    let program = elf(3, INTERP_BASE, INTERP_ENTRY, Some(PATH));
    let interpreter = read(&elf(3, INTERP_BASE, INTERP_ENTRY, None), 62).unwrap();
    let headers = read(&program, 62).unwrap()
        .position_program(PROGRAM_BASE..PROGRAM_BASE + 0x10000).unwrap();
    let plan = headers.exec_plan(Some((&interpreter, PROGRAM_BASE..PROGRAM_BASE + 0x10000)))
        .unwrap();
    let auxv: std::collections::BTreeMap<_, _> = plan.auxv(RANDOM, IDS).collect();
    let bias = PROGRAM_BASE - INTERP_BASE;
    assert_eq!(auxv[&0x6000_0000], bias);
    assert_eq!(auxv[&3], bias + INTERP_BASE + 64);
    assert_eq!(auxv[&9], bias + INTERP_ENTRY);
    assert_eq!(plan.auxv_size(), auxv.len() * 16);
}

#[test]
fn static_auxv_unchanged() {
    let headers = read(&elf(2, PROGRAM_BASE, PROGRAM_ENTRY, None), 62).unwrap();
    assert_eq!(headers.interpreter_path(), None);
    let plan = headers.exec_plan(None).unwrap();
    assert_eq!(plan.initial_pc(), PROGRAM_ENTRY);
    assert_eq!(
        plan.auxv(RANDOM, IDS).collect::<Vec<_>>(),
        [
            (9, PROGRAM_ENTRY),
            (3, PROGRAM_BASE + 64),
            (5, 1),
            (4, 56),
            (6, PAGE),
            (25, RANDOM),
            (11, 1000),
            (12, 1001),
            (13, 2000),
            (14, 2001),
            (23, 1),
            (0, 0),
        ]
    );
}
