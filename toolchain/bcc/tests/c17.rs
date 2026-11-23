use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use bcc::pp;

struct Source(PathBuf);

impl Source {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("breenix-bcc-{name}-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        Self(dir.join("input.c"))
    }

    fn compile(&self, source: &str, stage: &str) -> Output {
        fs::write(&self.0, source).unwrap();
        Command::new(env!("CARGO_BIN_EXE_bcc"))
            .args(["--target=x86_64", "-std=c17", stage, "-I"])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../include"))
            .arg(&self.0)
            .output()
            .unwrap()
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.0.parent().unwrap());
    }
}

fn success(output: Output) -> Vec<u8> {
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    output.stdout
}

#[test]
fn dr_correction_412() {
    // WG14 N2244 DR 412: after a taken branch, do not evaluate #elif.
    let source = pp::preprocess_source(
        Path::new("elif.c"),
        b"#if 1\nselected\n#elif 1 / 0\nunselected\n#endif\n",
        &pp::Options::default(),
    )
    .unwrap();
    let mut text = Vec::new();
    source.write(&mut text).unwrap();
    assert_eq!(text, b"selected\n");
}

#[test]
fn dr_correction_423() {
    // WG14 N2244 DR 423: casts and function results lose top-level
    // qualifiers. Inner qualifiers still belong to the pointed-to type.
    let source = Source::new("dr423");
    success(source.compile(
        r#"
int f(void);
const int f(void) { return 1; }
typedef int *pointer;
int *g(void);
const pointer g(void);
_Static_assert(_Generic((const int)1, int: 1, default: 0), "cast result");
_Static_assert(_Generic(f(), int: 1, default: 0), "function result");
_Static_assert(_Generic((const int *)0, const int *: 1, default: 0), "nested qualifier");
"#,
        "-fsyntax-only",
    ));
}

#[test]
fn dr_correction_444() {
    // WG14 N2244 DR 444: an alignment specifier can appear on a member.
    let path = Path::new("aligned-member.c");
    let options = pp::Options::default();
    let source = pp::preprocess_source(
        path,
        br#"
struct aligned { char prefix; _Alignas(16) char value; };
_Static_assert(_Alignof(struct aligned) == 16, "record alignment");
_Static_assert(__builtin_offsetof(struct aligned, value) == 16, "member offset");
"#,
        &options,
    )
    .unwrap();
    bcc::sema::analyze(&source, path, options.target).unwrap();
}

#[test]
fn dr_correction_465() {
    // WG14 N2244 DR 465: the pointed-to type determines the result,
    // including for a null pointer. Preserve evaluation of the argument.
    let source = Source::new("dr465");
    let code = r"
#include <stdatomic.h>
struct one_int { int value; };
static int calls;
static atomic_int *get_pointer(void) { ++calls; return (atomic_int *)0; }
int main(void) {
    _Atomic(struct one_int) record;
    if (!atomic_is_lock_free(get_pointer()) || calls != 1)
        return 1;
    if (!atomic_is_lock_free((atomic_int *)0))
        return 2;
    if (atomic_is_lock_free(&record))
        return 3;
    return 0;
}
";
    let assembly = source.0.with_extension("s");
    let object = source.0.with_extension("o");
    let executable = source.0.with_extension("elf");
    fs::write(&assembly, success(source.compile(code, "-S"))).unwrap();
    let bcc = Path::new(env!("CARGO_BIN_EXE_bcc"));
    success(
        Command::new(bcc.with_file_name("bas"))
            .args(["--target", "x86_64", "-o"])
            .arg(&object)
            .arg(&assembly)
            .output()
            .unwrap(),
    );
    let toolchain = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let script = fs::read_to_string(toolchain.join("../userspace/programs/linker.ld")).unwrap();
    let entry_script = source.0.with_extension("ld");
    fs::write(&entry_script, script.replace("ENTRY(_start)", "ENTRY(main)")).unwrap();
    success(
        Command::new(bcc.with_file_name("bld"))
            .arg("-T")
            .arg(&entry_script)
            .args(["--gc-sections", "-o"])
            .arg(&executable)
            .arg(&object)
            .output()
            .unwrap(),
    );
    let mut python = if cfg!(target_os = "macos") {
        let mut command = Command::new("arch");
        command.args(["-x86_64", "/usr/bin/python3"]);
        command
    } else {
        Command::new("python3")
    };
    let stdout = success(
        python
            .arg(toolchain.join("tests/bcc/return_42_exit_status.py"))
            .arg("--call-entry")
            .arg(executable)
            .output()
            .unwrap(),
    );
    assert!(stdout.is_empty());
}

#[test]
fn dr_correction_481() {
    // WG14 N2244 DR 481: determine the controlling type after lvalue,
    // array and function conversions, without evaluating the expression.
    let path = Path::new("generic-control.c");
    let options = pp::Options::default();
    let source = pp::preprocess_source(
        path,
        br#"
extern const int value;
extern int array[3];
extern int function(void);
_Static_assert(_Generic(value, int: 1, default: 0), "lvalue");
_Static_assert(_Generic(array, int *: 1, default: 0), "array");
_Static_assert(_Generic(function, int (*)(void): 1, default: 0), "function");
_Static_assert(_Generic(function(), int: 1, default: 0), "unevaluated");
"#,
        &options,
    )
    .unwrap();
    let unit = bcc::sema::analyze(&source, path, options.target).unwrap();
    assert!(unit.functions.iter().all(|function| function.body.is_none()));
}
