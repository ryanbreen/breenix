use std::fs;
use std::path::PathBuf;
use std::process::Command;

struct Source(PathBuf);

impl Source {
    fn new(name: &str, text: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("breenix-bcc-{name}-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let source = Self(dir.join("input.c"));
        fs::write(&source.0, text).unwrap();
        source
    }

    fn assembly(&self, target: &str, flags: &[&str]) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_bcc"))
            .args(["-S", target])
            .args(flags)
            .arg(&self.0)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty(), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.0.parent().unwrap());
    }
}

fn function<'a>(assembly: &'a str, name: &str) -> &'a str {
    let (_, body) = assembly.split_once(&format!("\n{name}:\n")).unwrap();
    body.split_once(&format!(".size {name},")).unwrap().0
}

#[test]
fn rounding_math_blocks_fp_fold() {
    // N1570 7.6.1 and F.8.2. The flag protects runtime arithmetic even
    // without a pragma, and FENV_ACCESS ON also works on its own.
    let source = Source::new(
        "runtime-rounding",
        r"
double third(void) { return 1.0 / 3.0; }
#pragma STDC FENV_ACCESS ON
double pragma_third(void) { return 1.0 / 3.0; }
",
    );
    for (target, division) in [("--target=x86_64", "divsd "), ("--target=aarch64", "fdiv ")] {
        let strict = source.assembly(target, &["-frounding-math"]);
        assert!(function(&strict, "third").contains(division), "{strict}");
        let default = source.assembly(target, &[]);
        assert!(
            function(&default, "pragma_third").contains(division),
            "{default}"
        );
    }
}

#[test]
fn rounding_math_preserves_fp_grouping() {
    // N1570 F.8.2. Changing the grouping can change which intermediate
    // rounds and therefore the result even under the default mode.
    let source = Source::new(
        "fp-grouping",
        r"
double left(double a, double b, double c) { return (a + b) - c; }
double right(double a, double b, double c) { return a + (b - c); }
",
    );
    for (target, add, sub) in [
        ("--target=x86_64", "addsd ", "subsd "),
        ("--target=aarch64", "fadd ", "fsub "),
    ] {
        let assembly = source.assembly(target, &["-frounding-math"]);
        let left = function(&assembly, "left");
        let right = function(&assembly, "right");
        assert!(left.split_once(add).unwrap().1.contains(sub), "{left}");
        assert!(right.split_once(sub).unwrap().1.contains(add), "{right}");
    }
}

#[test]
fn rounding_math_keeps_translation_time_initializers() {
    // N1570 6.6, F.8.4 and F.8.5. Binary64 rounds 1/3 to
    // 0x3fd5555555555555, and 2^53 + 1 rounds back to 2^53.
    let source = Source::new(
        "static-rounding",
        r#"
#pragma STDC FENV_ACCESS ON
_Static_assert((int)3.5 == 3, "integer constant expression");
static double d = 1.0 / 3.0;
static double grouped = ((0x1p53 + 1.0) - 0x1p53) * 2.0;
static double product = 1.5 * 3.0;
double result(void) { return d + grouped + product; }
"#,
    );
    for target in ["--target=x86_64", "--target=aarch64"] {
        let assembly = source.assembly(target, &["-frounding-math"]);
        for (name, bits) in [
            ("d", 0x3fd5_5555_5555_5555_u64),
            ("grouped", 0),
            ("product", 0x4012_0000_0000_0000),
        ] {
            let (_, data) = assembly.split_once(&format!("\n{name}:\n")).unwrap();
            let (directives, _) = data.split_once(&format!(".size {name}, 8\n")).unwrap();
            let mut bytes = Vec::new();
            for directive in directives.lines().map(str::trim) {
                if let Some(byte) = directive.strip_prefix(".byte ") {
                    bytes.push(byte.parse::<u8>().unwrap());
                } else if let Some(count) = directive.strip_prefix(".zero ") {
                    bytes.extend(std::iter::repeat_n(0, count.parse().unwrap()));
                } else {
                    panic!("unexpected data directive for {name}: {directive}");
                }
            }
            assert_eq!(bytes, bits.to_le_bytes(), "{target} {name}");
        }
    }
}

#[test]
fn constant_propagation_folds_local() {
    // N1570 6.3.1.3 and 6.5.5. The converted local value is 6.
    let source = Source::new(
        "propagate-local",
        r"
int folded(void) { unsigned char a = 262; int b = a * 7; return b + 1; }
int escaped(void) { int a = 6; int *p = &a; *p = 9; return a * 7; }
int changed(int p) { int a = 6; if (p) a = 9; return a * 7; }
double runtime(void) { double a = 1.0; return a / 3.0; }
",
    );
    let target = "--target=x86_64";
    let o0 = source.assembly(target, &["-O0", "-frounding-math"]);
    assert!(function(&o0, "folded").contains("imul "));
    let o2 = source.assembly(target, &["-O2", "-frounding-math"]);
    let folded = function(&o2, "folded");
    assert!(folded.contains("mov rax, 0x2b\n"), "{folded}");
    assert!(!folded.contains("imul "), "{folded}");
    for (name, operation) in [
        ("escaped", "imul "),
        ("changed", "imul "),
        ("runtime", "divsd "),
    ] {
        assert!(function(&o0, name).contains(operation), "{o0}");
        assert!(function(&o2, name).contains(operation), "{o2}");
    }
    assert_eq!(source.assembly(target, &["-O1", "-frounding-math"]), o0);
    assert_eq!(
        source.assembly(target, &["-O2", "-O0", "-frounding-math"]),
        o0
    );
    let invalid = Command::new(env!("CARGO_BIN_EXE_bcc"))
        .args(["-S", target, "-O3"])
        .arg(&source.0)
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert!(invalid.stdout.is_empty());
    assert!(String::from_utf8(invalid.stderr)
        .unwrap()
        .contains("unknown option '-O3'"));
}

#[test]
fn unreachable_block_removed() {
    // N1570 6.8.4.1 and 6.8.6.1. A goto may enter the unselected arm.
    let source = Source::new(
        "unreachable-block",
        r"
extern void dead(void);
extern void live(void);
void unreachable(void) { int zero = 0; if (zero) dead(); live(); return; dead(); }
void jumped(void) { goto entry; if (0) { entry: live(); } }
",
    );
    let target = "--target=x86_64";
    let o0 = source.assembly(target, &["-O0"]);
    assert_eq!(
        function(&o0, "unreachable")
            .matches("call dead@plt")
            .count(),
        2
    );
    let o2 = source.assembly(target, &["-O2"]);
    let reachable = function(&o2, "unreachable");
    assert!(!reachable.contains("call dead@plt"), "{reachable}");
    for assembly in [&o0, &o2] {
        for name in ["unreachable", "jumped"] {
            assert_eq!(function(assembly, name).matches("call live@plt").count(), 1);
        }
    }
}

#[test]
fn cse_keeps_volatile() {
    // N1570 5.1.2.3p6 and 6.7.3p7. The two volatile reads are observable.
    let source = Source::new(
        "local-cse",
        r"
volatile int observed;
int repeated(int x) { int first = x * 7; int second = x * 7; return first + second; }
int nested(int x) { return (x * 9 + 2) * (x * 9 + 2); }
int changed(int x) { int first = x * 7; x += observed; return first + x * 7; }
int read_twice(void) { return observed * 7 + observed * 7; }
",
    );
    let target = "--target=x86_64";
    let o0 = source.assembly(target, &["-O0"]);
    assert_eq!(function(&o0, "repeated").matches("imul ").count(), 2);
    assert_eq!(function(&o0, "nested").matches("imul ").count(), 3);
    let o2 = source.assembly(target, &["-O2"]);
    assert_eq!(function(&o2, "repeated").matches("imul ").count(), 1);
    assert_eq!(function(&o2, "nested").matches("imul ").count(), 2);
    for assembly in [&o0, &o2] {
        assert_eq!(function(assembly, "changed").matches("imul ").count(), 2);
        let volatile = function(assembly, "read_twice");
        assert_eq!(volatile.matches("[rip + observed]").count(), 2);
        assert_eq!(volatile.matches("imul ").count(), 2);
    }
}
