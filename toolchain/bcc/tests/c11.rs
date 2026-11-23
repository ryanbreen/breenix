use std::fs;
use std::path::Path;
use std::process::Command;

#[test]
fn constraint_paragraphs_located() {
    let toolchain = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    checked(
        Command::new("python3")
            .arg(toolchain.join("tests/bcc/constraint_paragraphs.py"))
            .arg(env!("CARGO_BIN_EXE_bcc")),
    );
}

#[test]
fn profile_macros_truthful() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus/c11/profile-macros.c");
    let scratch = Scratch(std::env::temp_dir().join(format!(
        "breenix-bcc-c11-{}-profile",
        std::process::id()
    )));
    fs::create_dir(&scratch.0).unwrap();
    let arm_fixture = scratch.0.join("arm-profile.c");
    // Mirror the guest fixture with the ARM exception from E25 ruling 47.
    fs::write(
        &arm_fixture,
        r#"
#if !defined(__STDC_NO_VLA__) || __STDC_NO_VLA__ != 1
#error expected __STDC_NO_VLA__ to be defined as 1
#endif
#if !defined(__STDC_NO_COMPLEX__) || __STDC_NO_COMPLEX__ != 1
#error expected __STDC_NO_COMPLEX__ to be defined as 1
#endif
#if defined(__STDC_IEC_559_COMPLEX__)
#error expected __STDC_IEC_559_COMPLEX__ to be absent
#endif
#if !defined(__STDC_NO_ATOMICS__) || __STDC_NO_ATOMICS__ != 1
#error expected __STDC_NO_ATOMICS__ to be defined as 1 on ARM
#endif
#if !defined(__STDC_NO_THREADS__) || __STDC_NO_THREADS__ != 1
#error expected __STDC_NO_THREADS__ to be defined as 1
#endif
int profile_macros_verified;
"#,
    )
    .unwrap();
    for target in ["--target=x86_64", "--target=aarch64"] {
        let result = Command::new(env!("CARGO_BIN_EXE_bcc"))
            .args([target, "-E"])
            .arg(if target == "--target=aarch64" {
                &arm_fixture
            } else {
                &fixture
            })
            .output()
            .unwrap();
        assert!(result.status.success(), "{target}: {result:?}");
        assert!(result.stderr.is_empty(), "{target}: {result:?}");
        let source = String::from_utf8(result.stdout).unwrap();
        assert!(
            source.lines().any(|line| line == "int profile_macros_verified ;"),
            "{target}: {source}"
        );
    }
}

#[test]
fn aarch64_defers_atomic_accesses() {
    use bcc::{codegen, pp, sema};

    let path = Path::new("atomic-arm.c");
    let options = pp::Options {
        target: pp::Target::Aarch64,
        ..pp::Options::default()
    };
    let mut mismatches = Vec::new();
    for source in [
        "_Atomic int object;\nint f(void) { return object; }",
        "_Atomic int object;\nvoid f(void) { object = 4; }",
        "_Atomic int object;\nint f(void) { return object++; }",
        "int f(_Atomic(int) *p) {\n return *p; }",
        "void f(_Atomic(int) *p) {\n *p = 4; }",
        "void f(_Atomic(int) *p) {\n *p += 4; }",
        "void f(_Atomic(int) *p) {\n ++*p; }",
        "int f(_Atomic(int) *p) {\n return __c11_atomic_load(p, 5); }",
        "int f(void) {\n _Atomic int local = 4; return 0; }",
        "extern _Atomic int object;\n_Atomic int object = 4;",
    ] {
        let input = pp::preprocess_source(path, source.as_bytes(), &options).unwrap();
        let unit = sema::analyze(&input, path, options.target).unwrap();
        let diagnostic = codegen::emit_assembly(&unit, path, options.target)
            .err()
            .map(|error| error.to_string());
        let located = diagnostic.as_deref().is_some_and(|message| {
            message
                .strip_prefix("atomic-arm.c:2:")
                .and_then(|rest| rest.split_once(": error: "))
                .is_some_and(|(column, reason)| {
                    column.parse::<usize>().is_ok() && reason.contains("ARM 25.9d")
                })
        });
        if !located {
            mismatches.push(format!("{source}: {diagnostic:?}"));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

// N1570 6.5.2.4, 6.5.16.2 and 7.17; Intel SDM Vol. 3, memory ordering.
#[test]
fn atomic_fetch_add_seq_cst() {
    let result = build_and_check(
        "atomic",
        r"
#include <stdatomic.h>
int main(void) {
    volatile _Atomic(unsigned char) byte;
    _Atomic short half = 6;
    atomic_int word;
    long numbers[4];
    long * _Atomic address = numbers;
    atomic_init(&byte, 255);
    atomic_init(&word, 19);
    int old = atomic_fetch_add(&word, 23);
    if (old != 19 || word != 42 || !atomic_is_lock_free(&word))
        return 1;
    if (byte++ != 255 || byte != 0 || --half != 5 || (half *= 3) != 15)
        return 2;
    if (atomic_fetch_sub(&half, 1) != 15 ||
        atomic_fetch_or(&half, 1) != 14 ||
        atomic_fetch_xor(&half, 4) != 15 ||
        atomic_fetch_and(&half, 9) != 11 || half != 9)
        return 3;
    if (atomic_fetch_add(&address, 2) != numbers ||
        (address -= 1) != numbers + 1 || address != numbers + 1)
        return 4;
    int expected = 0;
    if (atomic_compare_exchange_strong(&word, &expected, 10) || expected != 42)
        return 5;
    while (!atomic_compare_exchange_weak(&word, &expected, 10)) {
        if (expected != 42) return 6;
    }
    if (atomic_exchange(&word, 20) != 10)
        return 7;
    atomic_store(&word, 40);
    atomic_thread_fence(memory_order_seq_cst);
    atomic_signal_fence(memory_order_seq_cst);
    return atomic_load(&word) + 2;
}
",
    );
    let assembly = fs::read_to_string(result.0.join("c11.s")).unwrap();
    let (_, main) = assembly.split_once("\nmain:\n").expect("main label");
    let (main, _) = main.split_once("\n.size main,").expect("main size");
    assert!(
        main.lines()
            .any(|line| line.trim_start().starts_with("lock xadd dword ptr ")),
        "{main}"
    );
    assert!(!main.contains("__atomic_"), "{main}");
    assert_runs_to_42(result);
}

// Linux is a development executor. The bounded Rust driver links one
// bcc/bas object and the Cargo-built runtime for the same host triple.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn aggregate_atomic_cas_under_contention() {
    let scratch = Scratch(
        std::env::temp_dir().join(format!("breenix-bcc-c11-{}-contention", std::process::id())),
    );
    let work = &scratch.0;
    fs::create_dir(work).unwrap();
    let package = Path::new(env!("CARGO_MANIFEST_DIR"));
    let compiler = Path::new(env!("CARGO_BIN_EXE_bcc"));
    let bin = work.join("bin");
    fs::create_dir(&bin).unwrap();
    fs::copy(compiler.with_file_name("bas"), bin.join("as")).unwrap();
    let object = work.join("worker.o");
    checked(
        Command::new(compiler)
            .args(["-c", "-I"])
            .arg(package.join("../include"))
            .arg("-B")
            .arg(&bin)
            .arg("-o")
            .arg(&object)
            .arg(package.join("../tests/bcc/aggregate_atomic_worker.c")),
    );

    let cargo = env!("CARGO");
    let triple = "x86_64-unknown-linux-gnu";
    let target = work.join("target");
    checked(
        Command::new(cargo)
            .current_dir(package)
            .args(["build", "--offline", "--release", "-p", "brt", "--target", triple])
            .arg("--target-dir")
            .arg(&target),
    );
    let archive = target.join(triple).join("release/libbrt.a");
    // brt owns a no_std panic handler. Keep the finished archive in the
    // worker library so it does not collide with the thread driver's std.
    checked(
        Command::new("cc")
            .args([
                "-shared",
                "-Wl,-z,defs,-z,noexecstack,-Bsymbolic,--gc-sections,--exclude-libs,ALL",
                "-o",
            ])
            .arg(work.join("libatomic_worker.so"))
            .arg(object)
            .arg(archive),
    );
    fs::write(
        work.join("Cargo.toml"),
        "[package]\nname = \"atomic-threads\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\
         [[bin]]\nname = \"atomic-threads\"\npath = \"main.rs\"\n",
    )
    .unwrap();
    fs::write(
        work.join("main.rs"),
        include_str!("../../tests/bcc/aggregate_atomic_threads.rs"),
    )
    .unwrap();
    checked(
        Command::new(cargo)
            .current_dir(package)
            .args(["rustc", "--offline", "--target", triple, "--target-dir"])
            .arg(&target)
            .arg("--manifest-path")
            .arg(work.join("Cargo.toml"))
            .arg("--")
            .arg("-L")
            .arg(format!("native={}", work.display()))
            .arg("-l")
            .arg("atomic_worker"),
    );
    checked(
        Command::new("timeout")
            .env("LD_LIBRARY_PATH", work)
            .arg("10s")
            .arg(target.join(triple).join("debug/atomic-threads")),
    );
}

// N1570 6.7.9p17–22, 6.5.2.5p4–5, and 6.7.2.1p18.
#[test]
fn designated_init_last_wins() {
    let input = r"
struct Row { int cells[3]; int tail; };
union Value { long wide; struct Row row; };
int main(void) {
    int a[] = { [3] = 19, [1 + 1] = 7, [3] = 23, 5 };
    struct Row rows[3] = {
        [1].cells[1] = 4, 8, 9,
        [0] = { .tail = 44, .cells = { [2] = 17 } },
        [0].cells[2] = 6,
        [2].cells[2] = 10, [2] = { .tail = 42 },
    };
    if (sizeof(a) != 5 * sizeof(int) || a[0] || a[1] || a[2] != 7 || a[3] != 23 || a[4] != 5)
        return 1;
    if (rows[0].cells[0] || rows[0].cells[1] || rows[0].cells[2] != 6 || rows[0].tail != 44 ||
        rows[1].cells[0] || rows[1].cells[1] != 4 || rows[1].cells[2] != 8 || rows[1].tail != 9 ||
        rows[2].cells[2] || rows[2].tail != 42)
        return 2;
    union Value u = { .row.cells[2] = 44, .wide = 99, .row.tail = 21 };
    if (u.row.cells[0] || u.row.cells[2] || u.row.tail != 21)
        return 3;
    struct Row *p = &(struct Row){ .cells[1] = 13 };
    p->tail = (int[]){ [1] = 29 }[1];
    if (p->cells[0] || p->cells[1] != 13 || p->tail != 29)
        return 4;
    struct Packet { char kind; long words[]; };
    struct Packet packet = { .kind = 3 };
    if (sizeof(packet) != 8 || (char *)&packet.words - (char *)&packet != 8 || packet.kind != 3)
        return 5;
    return 42;
}
";
    let result = build_and_check("automatic", input);
    let executable = result.0.join("c11.elf");
    let toolchain = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");

    let mut python = if cfg!(target_os = "macos") {
        let mut command = Command::new("arch");
        command.args(["-x86_64", "/usr/bin/python3"]);
        command
    } else {
        Command::new("python3")
    };
    let result = python
        .arg(toolchain.join("tests/bcc/return_42_exit_status.py"))
        .arg("--call-entry")
        .arg(executable)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(42), "{result:?}");
    assert!(result.stdout.is_empty() && result.stderr.is_empty());
}

// N1570 6.5.2.5p5 and 6.6p7–9: this pointer is an address constant
// with the offset of the selected member of the unnamed static object.
#[test]
fn file_scope_compound_literal_has_static_storage() {
    let result = build_and_check(
        "static",
        r"
struct Pair { int x, y; };
static int *address = &(struct Pair){ .y = 42 }.y;
int main(void) { return *address; }
",
    );
    let bytes = fs::read(result.0.join("c11.elf")).unwrap();
    let elf = bobj::elf::Elf::parse(&bytes).unwrap();
    let index = (0..elf.section_count())
        .find(|&index| elf.section_name(index).unwrap() == b".data")
        .unwrap();
    let data = elf.section_data(index).unwrap().unwrap();
    let base = elf.section(index).unwrap().address;
    let offset = data
        .chunks_exact(8)
        .find_map(|bytes| {
            let address = u64::from_le_bytes(bytes.try_into().unwrap());
            usize::try_from(address.checked_sub(base)?)
                .ok()
                .filter(|&offset| offset < data.len())
        })
        .expect("relocated pointer to the compound literal");
    assert_eq!(&data[offset - 4..offset + 4], &[0, 0, 0, 0, 42, 0, 0, 0]);
}

#[test]
fn rejecting_flexible_member_initializers_names_the_member_kind() {
    use bcc::{pp, sema};
    let path = Path::new("flexible.c");
    let options = pp::Options::default();
    for initializer in ["{ 1, { 2 } }", "{ .tail[0] = 2 }"] {
        let source = format!(
            "struct S {{ int count; int tail[]; }}; int f(void) {{ struct S s = {initializer}; return s.count; }}"
        );
        let input = pp::preprocess_source(path, source.as_bytes(), &options).unwrap();
        let diagnostic = sema::analyze(&input, path, options.target)
            .unwrap_err()
            .to_string();
        assert!(
            diagnostic.starts_with("flexible.c:1:")
                && diagnostic.contains("cannot initialize a flexible array member"),
            "{diagnostic}"
        );
    }
}

struct Scratch(std::path::PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn build_and_check(case: &str, input: &str) -> Scratch {
    build_with_libraries(case, input, &[])
}

fn build_with_libraries(case: &str, input: &str, libraries: &[&Path]) -> Scratch {
    let scratch = Scratch(
        std::env::temp_dir().join(format!("breenix-bcc-c11-{}-{case}", std::process::id())),
    );
    let work = &scratch.0;
    fs::create_dir(work).unwrap();
    let source = work.join("c11.c");
    let assembly = work.join("c11.s");
    let object = work.join("c11.o");
    let executable = work.join("c11.elf");
    fs::write(&source, input).unwrap();
    let bcc = Path::new(env!("CARGO_BIN_EXE_bcc"));
    let toolchain = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let script = work.join("main.ld");
    fs::write(
        &script,
        fs::read_to_string(toolchain.join("../userspace/programs/linker.ld"))
            .unwrap()
            .replace("ENTRY(_start)", "ENTRY(main)"),
    )
    .unwrap();
    checked(
        Command::new(bcc)
            .args(["-S", "-I"])
            .arg(toolchain.join("include"))
            .arg("-I")
            .arg(toolchain.join("../libs/libbreenix-libc/include"))
            .arg("-o")
            .arg(&assembly)
            .arg(source),
    );
    checked(
        Command::new(bcc.with_file_name("bas"))
            .arg("-o")
            .arg(&object)
            .arg(assembly),
    );
    checked(
        Command::new(bcc.with_file_name("bld"))
            .arg("-T")
            .arg(&script)
            .args(["--gc-sections", "-o"])
            .arg(&executable)
            .arg(object)
            .args(libraries),
    );
    checked(
        Command::new(bcc.with_file_name("elfcheck"))
            .arg("--exec")
            .arg(&executable),
    );

    scratch
}

fn checked(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// N1570 6.5.1.1: type selection preserves the chosen expression's lvalue
// and evaluates neither the controlling expression nor unselected arms.
#[test]
fn generic_selects_compatible() {
    let result = build_and_check(
        "generic",
        r"
typedef int Number;
static int next(int *calls) { return ++*calls; }
int main(void) {
    const volatile Number n = 42;
    int calls = 0;
    int selected = 0;
    _Generic(n, int: selected, default: calls) = n;
    if (_Generic(next(&calls), Number: selected, default: next(&calls)) != 42)
        return 1;
    if (_Generic(1.0, int: next(&calls), default: 42) != 42 || calls)
        return 2;
    return selected;
}
",
    );
    assert_runs_to_42(result);
}

fn assert_runs_to_42(result: Scratch) {
    let toolchain = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut python = if cfg!(target_os = "macos") {
        let mut command = Command::new("arch");
        command.args(["-x86_64", "/usr/bin/python3"]);
        command
    } else {
        Command::new("python3")
    };
    let output = python
        .arg(toolchain.join("tests/bcc/return_42_exit_status.py"))
        .arg("--call-entry")
        .arg(result.0.join("c11.elf"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(42), "{output:?}");
    assert!(output.stdout.is_empty() && output.stderr.is_empty());
}

// N1570 6.7.5, 6.7.2.1p13, 6.7.10, 6.9.1 and 6.5.2.2p6.
#[test]
fn aligned_anonymous_members_and_old_style_parameters() {
    assert_runs_to_42(build_and_check(
        "declarations",
        r#"
struct Slot {
    char tag;
    union {
        struct { _Alignas(64) int value; int extra; };
        long other;
    };
    _Static_assert(_Alignof(long) >= _Alignof(char), "member assertion");
};
_Static_assert(_Alignof(struct Slot) == 64, "file assertion");
_Alignas(32) static int file = 2;
static _Noreturn void halt(void) { for (;;) {} }

static int old();
static int old(a, b, c, d, e, f, g, x)
    int b, a, c, d, e, f;
    short g;
    register float x;
{
    return a + b + c + d + e + f + g + (int)(x * 2);
}

int main(void) {
    _Static_assert(_Alignof(struct Slot[2]) == 64, "block assertion");
    static _Alignas(64) int local_static;
    _Alignas(struct Slot) int local = 5;
    struct Slot item = { .other = 99, .value = 8, 6 };
    struct Slot *p = &item;
    if (((unsigned long)&file % 32) || ((unsigned long)&local_static % 64) ||
        ((unsigned long)&local % 64) || ((unsigned long)&p->value % 64) ||
        p->value != 8 || p->extra != 6)
        return 1;
    if (0) halt();
    local_static = 3;
    return old(1, 2, 3, 4, 0, 0, (short)5, 1.5f)
        + file + local_static + local + item.value + item.extra;
}
"#,
    ));
}

#[test]
fn c11_constraints_have_located_diagnostics() {
    use bcc::{pp, sema};
    let path = Path::new("constraint.c");
    let options = pp::Options::default();
    for (line, source, message) in [
        (
            2,
            "int f(void) {\n return _Generic(0, long: 0); }",
            "no matching generic association",
        ),
        (
            2,
            "typedef int I;\nint f(void) { return _Generic(0, int: 0, I: 1); }",
            "duplicate compatible generic association",
        ),
        (
            2,
            "struct S {\n _Static_assert(0, \"member fails\"); int i; };",
            "member fails",
        ),
        (
            2,
            "int f(void) {\n _Static_assert(0, \"block fails\"); }",
            "block fails",
        ),
        (1, "_Static_assert(0, \"file fails\");", "file fails"),
    ] {
        let input = pp::preprocess_source(path, source.as_bytes(), &options).unwrap();
        let diagnostic = sema::analyze(&input, path, options.target)
            .unwrap_err()
            .to_string();
        assert!(
            diagnostic.starts_with(&format!("constraint.c:{line}:"))
                && diagnostic.contains(message),
            "{diagnostic}"
        );
    }
}

// N1570 5.2.4.2.2 and 6.4.4.2, AMD64 psABI 3.1.2, and AAPCS64 5.1.
#[test]
fn decimal_0_1_rounds_to_0x3fb999999999999a() {
    use bcc::{pp, sema};
    let path = Path::new("float-limits.c");
    let include = Path::new(env!("CARGO_MANIFEST_DIR")).join("../include");
    for (target, long_double) in [
        (
            pp::Target::X86_64,
            [
                0x3fc0_8000_0000_0000_0000_u128,
                0x0001_8000_0000_0000_0000,
                0x7ffe_ffff_ffff_ffff_ffff,
                1,
                0x3ffb_cccc_cccc_cccc_cccd,
                0x7fff_8000_0000_0000_0000,
            ],
        ),
        (
            pp::Target::Aarch64,
            [
                0x3f8f_0000_0000_0000_0000_0000_0000_0000,
                0x0001_0000_0000_0000_0000_0000_0000_0000,
                0x7ffe_ffff_ffff_ffff_ffff_ffff_ffff_ffff,
                1,
                0x3ffb_9999_9999_9999_9999_9999_9999_999a,
                0x7fff_0000_0000_0000_0000_0000_0000_0000,
            ],
        ),
    ] {
        let options = pp::Options {
            target,
            system_dirs: vec![include.clone()],
            ..pp::Options::default()
        };
        let input = pp::preprocess_source(
            path,
            br#"
#include <float.h>
_Static_assert(DBL_MANT_DIG == 53, "binary64 precision");
_Static_assert(FLT_EVAL_METHOD == 0, "evaluate at source precision");
#if FLT_RADIX != 2 || FLT_MANT_DIG != 24 || FLT_DIG != 6 || DBL_DIG != 15 || \
    FLT_DECIMAL_DIG != 9 || DBL_DECIMAL_DIG != 17 || FLT_MIN_EXP != -125 || \
    DBL_MIN_EXP != -1021 || FLT_MAX_EXP != 128 || DBL_MAX_EXP != 1024 || \
    FLT_MIN_10_EXP != -37 || DBL_MIN_10_EXP != -307 || \
    FLT_MAX_10_EXP != 38 || DBL_MAX_10_EXP != 308 || \
    FLT_HAS_SUBNORM != 1 || DBL_HAS_SUBNORM != 1 || LDBL_HAS_SUBNORM != 1 || \
    LDBL_MIN_EXP != -16381 || LDBL_MAX_EXP != 16384 || \
    LDBL_MIN_10_EXP != -4931 || LDBL_MAX_10_EXP != 4932
#error wrong floating type characteristics
#endif
#if defined(__x86_64__)
_Static_assert(LDBL_MANT_DIG == 64 && LDBL_DIG == 18 &&
               LDBL_DECIMAL_DIG == 21 && DECIMAL_DIG == 21, "x87 extended");
#else
_Static_assert(LDBL_MANT_DIG == 113 && LDBL_DIG == 33 &&
               LDBL_DECIMAL_DIG == 36 && DECIMAL_DIG == 36, "binary128");
#endif
float single[] = { FLT_EPSILON, FLT_MIN, FLT_MAX, FLT_TRUE_MIN };
double binary64[] = { DBL_EPSILON, DBL_MIN, DBL_MAX, DBL_TRUE_MIN, 0.1 };
#if defined(__x86_64__)
long double extended[] = { LDBL_EPSILON, LDBL_MIN, LDBL_MAX, LDBL_TRUE_MIN, 0.1L,
                           0x1.ffffffffffffffffp16383L };
#else
long double extended[] = { LDBL_EPSILON, LDBL_MIN, LDBL_MAX, LDBL_TRUE_MIN, 0.1L,
                           0x1.ffffffffffffffffffffffffffff8p16383L };
#endif
"#,
            &options,
        )
        .unwrap();
        let unit = sema::analyze(&input, path, target).unwrap();
        for (name, width, expected) in [
            ("single", 4, &[0x3400_0000, 0x0080_0000, 0x7f7f_ffff, 1][..]),
            (
                "binary64",
                8,
                &[
                    0x3cb0_0000_0000_0000,
                    0x0010_0000_0000_0000,
                    0x7fef_ffff_ffff_ffff,
                    1,
                    0x3fb9_9999_9999_999a,
                ][..],
            ),
            ("extended", 16, &long_double[..]),
        ] {
            let object = unit
                .static_objects
                .iter()
                .find(|global| global.object.name == name)
                .unwrap();
            let mut bytes = vec![0; expected.len() * width];
            for (&offset, value) in object.initializer.as_ref().unwrap() {
                let sema::ThreadData::Byte(byte) = value else {
                    panic!("{name} should contain floating data");
                };
                bytes[offset] = *byte;
            }
            for (actual, expected) in bytes.chunks_exact(width).zip(expected) {
                assert_eq!(
                    actual,
                    &expected.to_le_bytes()[..width],
                    "{target:?} {name}"
                );
            }
        }
    }
}

// N1570 5.2.4.2.2, 7.6, F.3 and F.8.4. Link Breenix's C ABI libc as
// tls.rs does and call the linked main with the existing host runner.
#[test]
fn flt_rounds_follows_fesetround() {
    let libc = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../libs/libbreenix-libc/target/x86_64-breenix-c/release/libc.a");
    assert_runs_to_42(build_with_libraries(
        "fenv",
        r"
#include <float.h>
#include <fenv.h>
#pragma STDC FENV_ACCESS ON
int main(void) {
    int modes[] = { FE_TONEAREST, FE_DOWNWARD, FE_UPWARD, FE_TOWARDZERO };
    int rounds[] = { 1, 3, 2, 0 };
    for (int i = 0; i < 4; ++i) {
        if (fesetround(modes[i]) || FLT_ROUNDS != rounds[i])
            return 1;
        if (feclearexcept(FE_ALL_EXCEPT))
            return 2;
        double sum = 1.0 + 0x1p-53;
        double expected = i == 2 ? 0x1.0000000000001p0 : 1.0;
        if (sum != expected || fetestexcept(FE_ALL_EXCEPT) != FE_INEXACT)
            return 3;
        volatile double halfway = 0x1.000001p0;
        float narrow = (float)halfway;
        if (narrow != (i == 2 ? 0x1.000002p0f : 1.0f))
            return 4;
        long double extended = 1.0L + 0x1p-64L;
        if (extended != (i == 2 ? 0x1.0000000000000002p0L : 1.0L))
            return 5;
        if ((int)-1.75L != -1 || FLT_ROUNDS != rounds[i])
            return 6;
    }
    if (fesetround(FE_TONEAREST) || feclearexcept(FE_ALL_EXCEPT))
        return 7;
    volatile double zero = 0.0;
    volatile double quotient = 1.0 / zero;
    return fetestexcept(FE_ALL_EXCEPT) == FE_DIVBYZERO ? 42 : 8;
}
",
        &[&libc],
    ));
}
