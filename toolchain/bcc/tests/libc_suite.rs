use std::path::Path;
use std::process::Command;

#[test]
fn headers_type_check() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let suite = root.join("userspace/libc-suite");
    let mut failures = Vec::new();
    for target in ["x86_64", "aarch64"] {
        for file in [
            "abi_check.c",
            "harness.c",
            "dirent_glob.c",
            "getopt_env.c",
            "locale_wchar.c",
            "pthread.c",
        ] {
            let output = Command::new(env!("CARGO_BIN_EXE_bcc"))
                .arg(format!("--target={target}"))
                .args([
                    "-fsyntax-only",
                    "-std=c17",
                    "-fno-builtin",
                    "-frounding-math",
                    "-O2",
                    "-Wall",
                    "-Werror",
                    "-I",
                ])
                .arg(&suite)
                .arg("-I")
                .arg(root.join("libs/libbreenix-libc/include"))
                .arg("-I")
                .arg(root.join("toolchain/include"))
                .arg(suite.join(file))
                .output()
                .unwrap();
            let matches = if target == "aarch64" && file == "pthread.c" {
                // E25 rulings 37 and 50: retire this placeholder when ARM 25.9d lands here.
                let header = root.join("toolchain/include/stdatomic.h");
                let expected = format!(
                    "{}:30:9: error: atomic types are deferred to ARM 25.9d\n",
                    header.display()
                );
                output.status.code() == Some(1) && output.stderr == expected.as_bytes()
            } else {
                output.status.success() && output.stderr.is_empty()
            };
            if !matches || !output.stdout.is_empty() {
                failures.push(format!(
                    "{target} {file}: {}\n{}{}",
                    output.status,
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn front_end_extensions_and_constraints() {
    let scratch = std::env::temp_dir().join(format!("bcc-libc-headers-{}", std::process::id()));
    std::fs::create_dir(&scratch).unwrap();
    let path = scratch.join("headers.c");
    let cases = [
        (
            "ABI type macros",
            "_Static_assert(_Generic((__SIZE_TYPE__)0, unsigned long: 1), \"size_t\");\n\
             _Static_assert(_Generic((__INTPTR_TYPE__)0, long: 1), \"intptr_t\");\n\
             _Static_assert(_Generic((__UINT8_TYPE__)0, unsigned char: 1), \"uint8_t\");\n\
             _Static_assert(_Generic((__WINT_TYPE__)0, unsigned int: 1), \"wint_t\");",
            None,
        ),
        (
            "attribute list and alternate keywords",
            "typedef struct { char a; int b; } __attribute__((packed, __aligned__(8))) Pair;\n\
             _Static_assert(_Alignof(Pair) == 8 && sizeof(Pair) == 8, \"record alignment\");\n\
             _Static_assert(__builtin_offsetof(Pair, b) == 1, \"packed member\");\n\
             void stop(void) __attribute__((__noreturn__));\n\
             int resume(void) __attribute__((returns_twice));\n\
             static __inline int read_value(int *__restrict p) { return __extension__ (*p); }",
            None,
        ),
        (
            "wide literals and sizeof array operand",
            "void f(void) {\n\
                 __WCHAR_TYPE__ msg[] = L\"H\" \"i\";\n\
                 char buffer[sizeof msg];\n\
                 _Static_assert(sizeof L\"\\u03b1\" == 8, \"wide code unit and terminator\");\n\
                 _Static_assert(sizeof buffer == 12, \"array operand of sizeof\");\n\
             }",
            None,
        ),
        (
            "null pointer constant",
            "void invoke(void (*f)(void));\n\
             void f(void) { invoke((void *)(3 - 3)); }",
            None,
        ),
        (
            "ordinary void pointer",
            "void invoke(void (*f)(void));\n\
             void f(void *p) { invoke(p); }",
            Some("headers.c:2:"),
        ),
        (
            "nonconstant zero cast",
            "void invoke(void (*f)(void));\n\
             void f(void) { invoke((void *)(0, 0)); }",
            Some("headers.c:2:"),
        ),
        (
            "unsupported attribute",
            "void f(void) __attribute__((unused));",
            Some("headers.c:1:29: error:"),
        ),
    ];
    let mut failures = Vec::new();
    for target in ["x86_64", "aarch64"] {
        for (name, source, error) in cases {
            std::fs::write(&path, source).unwrap();
            let output = Command::new(env!("CARGO_BIN_EXE_bcc"))
                .arg(format!("--target={target}"))
                .args(["-std=c17", "-fsyntax-only"])
                .arg(&path)
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr);
            let matches = match error {
                Some(location) => !output.status.success() && stderr.contains(location),
                None => output.status.success() && stderr.is_empty(),
            };
            if !matches || !output.stdout.is_empty() {
                failures.push(format!("{target} {name}: {}\n{stderr}", output.status));
            }
        }
    }
    std::fs::remove_dir_all(&scratch).unwrap();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn every_tu_links_at_o0() {
    suite_recipe("x86_64");
}

#[test]
fn arm_translation_units_at_o0() {
    suite_recipe("aarch64");
}

fn suite_recipe(target: &str) {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/bcc/libc_suite.py");
    let output = Command::new("python3")
        .arg(script)
        .arg(env!("CARGO_BIN_EXE_bcc"))
        .arg(target)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    print!("{}", String::from_utf8_lossy(&output.stdout));
}
