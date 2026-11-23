use std::fs;
use std::path::Path;
use std::process::Command;

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
            .args(["-S", "-o"])
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
            .arg(object),
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
