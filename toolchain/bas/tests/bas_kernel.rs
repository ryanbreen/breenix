#![forbid(unsafe_code)]

use bas::assemble;

#[test]
fn macro_expands_in_place() {
    let source = b"\
bits 16
ap_trampoline_start: db 0
bits 32
db 0
bits 64
%define REPEATS 2
%define OFF(label) ((label) - ap_trampoline_start)
%macro INTERRUPT_ENTRY 2
%1:
    jmp .x
    times REPEATS db %2
.x: ret
    times 0 nop
%endmacro
INTERRUPT_ENTRY timer_interrupt_entry, 7
db 99
INTERRUPT_ENTRY reschedule_interrupt_entry, 9
.size timer_interrupt_entry, OFF(timer_interrupt_entry.x)
";
    let expanded = b"\
ap_trampoline_start: db 0
db 0
timer_interrupt_entry:
    jmp timer_interrupt_entry.x
    db 7
    db 7
timer_interrupt_entry.x: ret
db 99
reschedule_interrupt_entry:
    jmp reschedule_interrupt_entry.x
    db 9
    db 9
reschedule_interrupt_entry.x: ret
.size timer_interrupt_entry, timer_interrupt_entry.x - ap_trampoline_start
";
    assert_eq!(assemble(source).unwrap(), assemble(expanded).unwrap());
}

#[test]
fn undefined_macro_is_error_not_panic() {
    let error = assemble(b"bits 64\n    MISSING 1").unwrap_err();
    assert_eq!(
        error.to_string(),
        "2:5: unsupported instruction or directive"
    );
}

#[test]
fn wrong_macro_arity_is_error_not_panic() {
    for (source, expected) in [
        (
            "%macro ENTRY 2\n%endmacro\n    ENTRY one",
            "3:5: wrong number of macro arguments",
        ),
        (
            "%define OFF(label) label\n    dd OFF(one, two)",
            "2:8: wrong number of macro arguments",
        ),
    ] {
        assert_eq!(
            assemble(source.as_bytes()).unwrap_err().to_string(),
            expected
        );
    }
}

#[test]
fn unterminated_macro_is_error_not_panic() {
    let error = assemble(b"; comment\n  %macro ENTRY 1\n%1: ret\n").unwrap_err();
    assert_eq!(error.to_string(), "2:3: unterminated %macro");
}

#[test]
fn privileged_64_encodings() {
    // Intel SDM 325462-082US, instruction tables and addressing tables.
    let rows: &[(&str, &[u8])] = &[
        ("swapgs", &[0x0f, 0x01, 0xf8]),            // Vol. 2B, SWAPGS, 4-685
        ("iretq", &[0x48, 0xcf]),                   // Vol. 2A, IRETQ, 3-539
        ("o64 sysret", &[0x48, 0x0f, 0x07]),        // Vol. 2B, SYSRET, 4-697
        ("mov rax, cr0", &[0x0f, 0x20, 0xc0]),      // Vol. 2B, MOV control registers, 4-39
        ("mov rdx, cr3", &[0x0f, 0x20, 0xda]),      // Vol. 2B, MOV control registers, 4-39
        ("mov r8, cr4", &[0x41, 0x0f, 0x20, 0xe0]), // Vol. 2B, MOV control registers, 4-39
        ("mov cr0, rax", &[0x0f, 0x22, 0xc0]),      // Vol. 2B, MOV control registers, 4-39
        ("mov cr3, rdx", &[0x0f, 0x22, 0xda]),      // Vol. 2B, MOV control registers, 4-39
        ("mov cr4, r8", &[0x41, 0x0f, 0x22, 0xe0]), // Vol. 2B, MOV control registers, 4-39
        ("wrmsr", &[0x0f, 0x30]),                   // Vol. 2D, WRMSR, 6-9
        ("rdmsr", &[0x0f, 0x32]),                   // Vol. 2B, RDMSR, 4-537
        ("out dx, al", &[0xee]),                    // Vol. 2B, OUT, 4-177
        ("hlt", &[0xf4]),                           // Vol. 2A, HLT, 3-489
        ("cli", &[0xfa]),                           // Vol. 2A, CLI, 3-167
        ("cld", &[0xfc]),                           // Vol. 2A, CLD, 3-159
        ("sete al", &[0x0f, 0x94, 0xc0]),           // Vol. 2B, SETcc, 4-611
        ("sete dil", &[0x40, 0x0f, 0x94, 0xc7]),    // Vol. 2B, SETcc, 4-611
        ("sete r8b", &[0x41, 0x0f, 0x94, 0xc0]),    // Vol. 2B, SETcc, 4-611
        (
            "mov [gs:40], rsp",
            &[0x65, 0x48, 0x89, 0x24, 0x25, 40, 0, 0, 0],
        ), // Vol. 2A, MOV and Tables 2-2, 2-3
        (
            "mov rax, qword [gs:80]",
            &[0x65, 0x48, 0x8b, 0x04, 0x25, 80, 0, 0, 0],
        ), // Vol. 2A, MOV and Tables 2-2, 2-3
    ];
    for &(source, expected) in rows {
        let object = assemble(source.as_bytes()).unwrap_or_else(|e| panic!("{source}: {e}"));
        let elf = bobj::elf::Elf::parse(&object).unwrap();
        let section = (0..elf.section_count())
            .find(|&i| elf.section_name(i).unwrap() == b".text")
            .unwrap();
        assert_eq!(
            elf.section_data(section).unwrap().unwrap(),
            expected,
            "{source}"
        );
    }
}

#[test]
fn malformed_privileged_forms_are_located() {
    for source in [
        "mov cr2, rax",
        "mov cr3, eax",
        "mov rax, cr5",
        "mov cr3, [rax]",
        "out dx, ax",
        "o64 nop",
        "swapgs rax",
        "mov rax, [gs:fs:40]",
    ] {
        let error = assemble(source.as_bytes()).unwrap_err().to_string();
        assert!(error.starts_with("1:"), "{source}: {error}");
    }
}

#[test]
fn kernel_entry_sources_assemble() {
    for (name, source) in [
        ("syscall", include_bytes!("../../../kernel/src/syscall/entry.asm").as_slice()),
        ("timer", include_bytes!("../../../kernel/src/interrupts/timer_entry.asm").as_slice()),
        ("breakpoint", include_bytes!("../../../kernel/src/interrupts/breakpoint_entry.asm").as_slice()),
    ] {
        assemble(source).unwrap_or_else(|error| panic!("{name}: {error}"));
    }
}
