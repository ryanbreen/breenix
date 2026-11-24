use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[cfg(target_arch = "aarch64")]
mod support;

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

    #[cfg(target_arch = "aarch64")]
    fn aarch64_check_at_both_levels(&self, flags: &[&str], inputs: &[&str]) {
        let source = fs::read_to_string(&self.0).unwrap();
        for level in ["--cc-flag=-O0", "--cc-flag=-O2"] {
            let mut args = vec![level];
            args.extend_from_slice(flags);
            args.extend_from_slice(inputs);
            args.push(&source);
            support::aarch64_check_exit(&args);
        }
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
fn zero_bitmask_removes_unreachable_branch() {
    // N1570 6.5.10 and 6.8.4.1. A constant mask disables the branch.
    let source = Source::new(
        "zero-bitmask",
        r"
extern void unavailable(void);
extern volatile unsigned input;
int plain(unsigned features) {
    if (features & 0u) unavailable();
    return 42;
}
int volatile_read(void) {
    if (input & 0u) unavailable();
    return 42;
}
",
    );
    let assembly = source.assembly("--target=x86_64", &["-O2"]);
    assert!(!function(&assembly, "plain").contains("unavailable"), "{assembly}");
    assert!(function(&assembly, "volatile_read").contains("input"), "{assembly}");
}

#[test]
fn constant_conditional_discards_unselected_call() {
    // N1570 6.5.15p4 governs which conditional operand is evaluated.
    let source = Source::new(
        "constant-conditional",
        r"
extern int unavailable(void);
int disabled(int flags) { return 0 && flags ? unavailable() : 42; }
int enabled(void) { return 1 ? 42 : unavailable(); }
",
    );
    let assembly = source.assembly("--target=x86_64", &["-O2"]);
    for name in ["disabled", "enabled"] {
        let body = function(&assembly, name);
        assert!(!body.contains("unavailable") && body.contains("0x2a"), "{body}");
    }
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
    #[cfg(target_arch = "aarch64")]
    {
        // DDI 0487 M.b FPCR.RMode [23:22] = 01 rounds toward +infinity.
        // Preserve the caller's FPCR and the integer callee-saved registers.
        let assembly = r"
.section .text
.global round_up
round_up:
    stp x19, x30, [sp, #-16]!
    mrs x19, fpcr
    mov x10, #3
    lsl x10, x10, #22
    bic x9, x19, x10
    orr x9, x9, #0x400000
    .long 0xd51b4409
    blr x0
    .long 0xd51b4413
    ldp x19, x30, [sp], #16
    ret
";
        source.aarch64_check_at_both_levels(
            &["--cc-flag=-frounding-math"],
            &[
                "--asm",
                assembly,
                r"
double third(void);
double pragma_third(void);
double round_up(double (*value)(void));
int main(void) {
    return round_up(third) == 0x1.5555555555556p-2 &&
           round_up(pragma_third) == 0x1.5555555555556p-2 ? 42 : 1;
}
",
            ],
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
    #[cfg(target_arch = "aarch64")]
    source.aarch64_check_at_both_levels(
        &["--cc-flag=-frounding-math"],
        &[r"
int folded(void);
int escaped(void);
int changed(int p);
double runtime(void);
int main(void) {
    return folded() == 43 && escaped() == 63 && changed(0) == 42 &&
           changed(1) == 63 && runtime() == 0x1.5555555555555p-2 ? 42 : 1;
}
"],
    );
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
    #[cfg(target_arch = "aarch64")]
    source.aarch64_check_at_both_levels(
        &[],
        &[r"
static int calls;
void unreachable(void);
void jumped(void);
void dead(void) { calls += 100; }
void live(void) { ++calls; }
int main(void) {
    unreachable();
    jumped();
    return calls == 2 ? 42 : 1;
}
"],
    );
}

#[test]
fn disabled_block_with_internal_goto_is_unreachable() {
    // N1570 6.8.6.1 permits an entry by goto from outside the block.
    // N1570 6.8.4.2 permits entry by the enclosing switch.
    let source = Source::new(
        "internal-goto",
        r"
extern void unavailable(void);
extern void live(void);
void internal(int p) {
    if (0) {
        if (p) goto repeat;
        unavailable();
    repeat:
        unavailable();
    }
}
void external(void) {
    goto entry;
    if (0) { entry: live(); }
}
void dispatched(int p) {
    switch (p) {
        if (0) { case 42: live(); }
    }
}
",
    );
    let assembly = source.assembly("--target=x86_64", &["-O2"]);
    let internal = function(&assembly, "internal");
    assert!(!internal.contains("unavailable"), "{internal}");
    for name in ["external", "dispatched"] {
        assert!(function(&assembly, name).contains("call live@plt"), "{assembly}");
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
    #[cfg(target_arch = "aarch64")]
    {
        for level in ["-O0", "-O2"] {
            let arm = source.assembly("--target=aarch64", &[level]);
            let volatile = function(&arm, "read_twice");
            assert_eq!(volatile.matches(":lo12:observed").count(), 2, "{volatile}");
        }
        source.aarch64_check_at_both_levels(
            &[],
            &[r"
extern volatile int observed;
int repeated(int x);
int nested(int x);
int changed(int x);
int read_twice(void);
int main(void) {
    observed = 5;
    return repeated(3) == 42 && nested(3) == 841 && changed(3) == 77 &&
           read_twice() == 70 ? 42 : 1;
}
"],
        );
    }
}
