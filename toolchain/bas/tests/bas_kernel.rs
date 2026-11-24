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
    assert_eq!(error.to_string(), "2:5: unsupported instruction or directive");
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
        assert_eq!(assemble(source.as_bytes()).unwrap_err().to_string(), expected);
    }
}

#[test]
fn unterminated_macro_is_error_not_panic() {
    let error = assemble(b"; comment\n  %macro ENTRY 1\n%1: ret\n").unwrap_err();
    assert_eq!(error.to_string(), "2:3: unterminated %macro");
}
