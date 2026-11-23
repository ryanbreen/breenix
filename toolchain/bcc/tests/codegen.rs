use std::fs;
use std::path::Path;
use std::process::Command;

use bcc::{codegen, pp, sema};

#[test]
fn double_literal_bits() {
    // N1570 6.4.4.2 and Annex F.5. These binary64 encodings follow from
    // rounding the exact decimal or hexadecimal rational to nearest/even.
    let cases = [
        ("0.1", 0x3fb9_9999_9999_999au64),
        ("1e23", 0x44b5_2d02_c7e1_4af6),
        ("9007199254740993.0", 0x4340_0000_0000_0000),
        ("0x1p-1074", 0x0000_0000_0000_0001),
        ("0x1.00000000000008p0", 0x3ff0_0000_0000_0000),
        ("0x1.00000000000018p0", 0x3ff0_0000_0000_0002),
    ];
    let path = Path::new("double-literal.c");
    let options = pp::Options {
        target: pp::Target::X86_64,
        ..pp::Options::default()
    };
    for (literal, bits) in cases {
        let c = format!("double value(void) {{ return {literal}; }}");
        let input = pp::preprocess_source(path, c.as_bytes(), &options).unwrap();
        let unit = sema::analyze(&input, path, options.target).unwrap();
        let assembly = codegen::emit_assembly(&unit, path, options.target).unwrap();
        let load = format!("    mov rax, 0x{bits:x}\n    movq xmm0, rax\n");
        assert!(
            assembly.contains(&load),
            "{literal} expected {bits:#018x}\n{assembly}"
        );
    }
}

#[test]
fn long_double_literal_bits() {
    // Intel SDM Vol. 1 §4.2.2/Table 4-3 and Fig. 8-13. The first
    // eight bytes contain the explicit integer bit and 63 fraction bits;
    // the next two contain the sign and the exponent biased by 16383.
    let cases = [
        ("0.1L", 0xcccc_cccc_cccc_cccdu64, 0x3ffb),
        ("18446744073709551615.0l", 0xffff_ffff_ffff_ffff, 0x403e),
        ("0x1.0000000000000001p0L", 0x8000_0000_0000_0000, 0x3fff),
        ("0x1.0000000000000003p0L", 0x8000_0000_0000_0002, 0x3fff),
        ("0x1p-16445L", 1, 0),
        ("0x0.ffffffffffffffffp-16382L", 0x8000_0000_0000_0000, 1),
    ];
    let path = Path::new("long-double-literal.c");
    let options = pp::Options {
        target: pp::Target::X86_64,
        ..pp::Options::default()
    };
    for (literal, significand, sign_exponent) in cases {
        let c = format!("long double value(void) {{ return {literal}; }}");
        let input = pp::preprocess_source(path, c.as_bytes(), &options).unwrap();
        let unit = sema::analyze(&input, path, options.target).unwrap();
        let assembly = codegen::emit_assembly(&unit, path, options.target).unwrap();
        let words = format!("    .quad 0x{significand:x}\n    .word 0x{sign_exponent:x}\n");
        assert!(
            assembly.contains(&words),
            "{literal} expected {words}\n{assembly}"
        );
    }
}

#[test]
fn aarch64_binary128_literal_bits() {
    // IEEE 754-2019 §3.4, Table 3.5: 15 exponent and 112 fraction bits.
    // The repeating expansion of 1/10 rounds up in the last fraction bit.
    let cases = [
        ("0.1L", 0x3ffb_9999_9999_9999_9999_9999_9999_999au128),
        ("0x1.0000000000000000000000000001p0L", 0x3fff_0000_0000_0000_0000_0000_0000_0001),
        ("0x1p-16494L", 1),
        ("0x1.ffffffffffffffffffffffffffffp16383L", 0x7ffe_ffff_ffff_ffff_ffff_ffff_ffff_ffff),
    ];
    let path = Path::new("binary128-literal.c");
    let options = pp::Options { target: pp::Target::Aarch64, ..pp::Options::default() };
    for (literal, bits) in cases {
        let c = format!("long double value(void) {{ return {literal}; }}");
        let input = pp::preprocess_source(path, c.as_bytes(), &options).unwrap();
        let unit = sema::analyze(&input, path, options.target).unwrap();
        let assembly = codegen::emit_assembly(&unit, path, options.target).unwrap();
        let words = format!("    .quad 0x{:x}\n    .quad 0x{:x}\n", bits as u64, bits >> 64);
        assert!(assembly.contains(&words), "{literal} expected {words}\n{assembly}");
    }
}

#[test]
fn bcc_has_no_host_float_parse() {
    let mut directories = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                directories.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let source = fs::read_to_string(&path).unwrap();
                for forbidden in ["parse::<f64>", "as f64", "f64::from"] {
                    assert!(
                        !source.contains(forbidden),
                        "{} contains {forbidden:?}",
                        path.display()
                    );
                }
            }
        }
    }
}

// Build bas, bld and elfcheck in the same Cargo target/profile as bcc before
// running this test, and build libbreenix-libc for x86_64-breenix-c in release.
#[test]
fn return_42_exit_status() {
    let toolchain = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let bcc = Path::new(env!("CARGO_BIN_EXE_bcc"));
    let bas = bcc.with_file_name("bas");
    let bld = bcc.with_file_name("bld");
    let elfcheck = bcc.with_file_name("elfcheck");
    let libc = toolchain.join("../libs/libbreenix-libc/target/x86_64-breenix-c/release/libc.a");
    for input in [&bas, &bld, &elfcheck, &libc] {
        assert!(input.is_file(), "build the test input {}", input.display());
    }

    let mut python = if cfg!(target_os = "macos") {
        let mut command = Command::new("arch");
        command.args(["-x86_64", "/usr/bin/python3"]);
        command
    } else {
        Command::new("python3")
    };
    let result = python
        .arg(toolchain.join("tests/bcc/return_42_exit_status.py"))
        .arg("--bcc")
        .arg(bcc)
        .arg("--bas")
        .arg(bas)
        .arg("--bld")
        .arg(bld)
        .arg("--elfcheck")
        .arg(elfcheck)
        .arg("--libc")
        .arg(libc)
        .output()
        .expect("run the codegen harness with x86-64 Python");
    assert!(
        result.status.success(),
        "{}\n{}{}",
        result.status,
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr),
    );
}

#[cfg(target_arch = "aarch64")]
#[test]
fn aarch64_return_42_exit_status() {
    let toolchain = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let bcc = Path::new(env!("CARGO_BIN_EXE_bcc"));
    let result = Command::new("python3")
        .arg(toolchain.join("tests/bcc/aarch64_return_42_exit_status.py"))
        .args([bcc, &bcc.with_file_name("bas"), &bcc.with_file_name("bld")])
        .output()
        .expect("run the ARM64 codegen harness with native Python");
    assert!(
        result.status.success(),
        "{}\n{}{}",
        result.status,
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr),
    );
}

#[cfg(target_arch = "aarch64")]
#[test]
fn aarch64_narrows_promoted_identifier_list_float() {
    // N1570 6.5.2.2p6 and 6.9.1p10. Exercise both a SIMD register
    // argument and the first floating argument spilled to the stack.
    aarch64_check_exit(&[
        r"
static int old(a, b, c, d, e, f, g, h, i)
    float a, b, c, d, e, f, g, h, i;
{
    if (a != 1.5f) return 1;
    if (h != 2.5f) return 2;
    if (i != 3.5f) return 3;
    return 42;
}
int main(void) {
    return old(1.5f, 0.f, 0.f, 0.f, 0.f, 0.f, 0.f, 2.5f, 3.5f);
}
",
    ]);
}

#[cfg(target_arch = "aarch64")]
#[test]
fn aarch64_normalizes_promoted_identifier_list_bool() {
    // N1570 6.3.1.2p1, 6.5.2.2p6 and 6.9.1p10. Neither the register
    // nor the stack argument can be converted by truncating its low byte.
    aarch64_check_exit(&[r"
static int old(a, b, c, d, e, f, g, h, i)
    _Bool a, i;
    int b, c, d, e, f, g, h;
{
    if (a != 1) return 1;
    if (i != 1) return 2;
    return 42;
}
int main(void) {
    return old(2, 0, 0, 0, 0, 0, 0, 0, 256);
}
"]);
}

#[cfg(target_arch = "aarch64")]
#[test]
fn aarch64_links_and_runs_initialized_file_scope_objects() {
    // N1570 6.7.9 and 6.9.2. Cross a translation-unit boundary for
    // external data and read a privately bound array in its defining file.
    aarch64_check_exit(&[
        r"
extern int global;
int read_other(void);
int main(void) {
    if (global != 40 || read_other() != 2) return 1;
    global += read_other();
    return global;
}
",
        r"
int global = 40;
static int private[2] = { 1, 2 };
int read_other(void) { return private[1]; }
",
    ]);
}

#[cfg(target_arch = "aarch64")]
#[test]
fn aarch64_c11_objects_link_and_run() {
    // N1570 6.5.2.5p5, 6.7.5 and 6.7.9. The ninth argument also
    // checks that stack arguments survive realignment (AAPCS64 §6.8.2).
    aarch64_check_exit(&[
        r"
int static_literal(void);
int static_local(void);

static int aligned(long a, long b, long c, long d, long e,
                   long f, long g, long h, long i) {
    _Alignas(32) int value = 17;
    if (((unsigned long)&value & 31) != 0) return 1;
    if (i != 9 || a + b + c + d + e + f + g + h != 36) return 2;
    return value;
}

int main(void) {
    if (aligned(1, 2, 3, 4, 5, 6, 7, 8, 9) != 17) return 3;
    if (static_literal() != 20) return 4;
    if (static_local() != 10 || static_local() != 11) return 5;
    int sum = 0;
    for (int i = 1; i <= 3; ++i)
        sum += ((int[]){i, i + 1})[1];
    return 33 + sum;
}
",
        r"
static int *value = (int[]){ 2, 19 } + 1;
int static_literal(void) { return ++*value; }
",
        r"
int static_local(void) {
    static int values[2] = { 8, 9 };
    static int *value = &values[1];
    return ++*value;
}
",
    ]);
}

#[cfg(target_arch = "aarch64")]
#[test]
fn aarch64_flt_rounds_tracks_fpcr() {
    // DDI 0487 M.b FPCR.RMode [23:22]; N1570 5.2.4.2.2.
    // Save and restore the entire control register even on a mismatch.
    let mut assembly = String::from(".text\n.global main\nmain:\n    stp x19, x30, [sp, #-16]!\n    mrs x19, fpcr\n    mov x10, #3\n    lsl x10, x10, #22\n");
    for (mode, expected) in [(0, 1), (1, 2), (2, 3), (3, 0)] {
        assembly.push_str(&format!("    bic x9, x19, x10\n    mov x11, #{mode}\n    lsl x11, x11, #22\n    orr x9, x9, x11\n    .long 0xd51b4409\n    bl rounds\n    cmp w0, #{expected}\n    b.ne .Lfailure\n    mov x10, #3\n    lsl x10, x10, #22\n"));
    }
    assembly.push_str("    mov w0, #42\n    b .Lrestore\n.Lfailure:\n    mov w0, #1\n.Lrestore:\n    .long 0xd51b4413\n    ldp x19, x30, [sp], #16\n    ret\n");
    let toolchain = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let bcc = Path::new(env!("CARGO_BIN_EXE_bcc"));
    let result = Command::new("python3")
        .arg(toolchain.join("tests/bcc/aarch64_return_42_exit_status.py"))
        .args([bcc, &bcc.with_file_name("bas"), &bcc.with_file_name("bld")])
        .args(["--asm", &assembly, "int rounds(void) { return __builtin_flt_rounds(); }"])
        .output().expect("run native FPCR harness");
    assert!(result.status.success(), "{}\n{}{}", result.status,
        String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
}

#[cfg(target_arch = "aarch64")]
fn aarch64_check_exit(sources: &[&str]) {
    let toolchain = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let bcc = Path::new(env!("CARGO_BIN_EXE_bcc"));
    let result = Command::new("python3")
        .arg(toolchain.join("tests/bcc/aarch64_return_42_exit_status.py"))
        .args([bcc, &bcc.with_file_name("bas"), &bcc.with_file_name("bld")])
        .args(sources)
        .output()
        .expect("run the ARM64 codegen harness with native Python");
    assert!(
        result.status.success(),
        "{}\n{}{}",
        result.status,
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr),
    );
}

#[test]
fn sysv_scalar_calls() {
    let toolchain = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let bcc = Path::new(env!("CARGO_BIN_EXE_bcc"));
    let mut python = if cfg!(target_os = "macos") {
        let mut command = Command::new("arch");
        command.args(["-x86_64", "/usr/bin/python3"]);
        command
    } else {
        Command::new("python3")
    };
    let result = python
        .arg(toolchain.join("tests/bcc/scalar_calls.py"))
        .arg(bcc)
        .arg(bcc.with_file_name("bas"))
        .arg(bcc.with_file_name("bld"))
        .output()
        .expect("run the scalar ABI harness with x86-64 Python");
    assert!(
        result.status.success(),
        "{}\n{}{}",
        result.status,
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr),
    );
}
