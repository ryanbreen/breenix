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
