use std::fs;
use std::path::Path;
use std::process::Command;

#[test]
fn float_classification_constants() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let bcc = Path::new(env!("CARGO_BIN_EXE_bcc"));
    let scratch = std::env::temp_dir().join(format!("bcc-builtins-{}", std::process::id()));
    fs::create_dir(&scratch).unwrap();
    let result = float_constants(bcc, &root, &scratch);
    fs::remove_dir_all(scratch).unwrap();
    result.unwrap();
}

fn float_constants(bcc: &Path, root: &Path, scratch: &Path) -> Result<(), String> {
    let source = scratch.join("constants.c");
    fs::write(
        &source,
        r#"
#include <math.h>
float inf = INFINITY;
float huge_f = HUGE_VALF;
double huge_d = HUGE_VAL;
long double huge_l = HUGE_VALL;
float quiet_nan = NAN;
double promoted_inf = INFINITY;
double promoted_nan = NAN;
float payload = __builtin_nanf("0x123");
"#,
    )
    .unwrap();
    for (target, long_bits) in [
        ("x86_64", 0x7fff_8000_0000_0000_0000u128),
        ("aarch64", 0x7fff_0000_0000_0000_0000_0000_0000_0000),
    ] {
        let assembly = scratch.join("constants.s");
        let object = scratch.join("constants.o");
        let compile = Command::new(bcc)
            .args([format!("--target={target}"), "-std=c17".into(), "-S".into()])
            .arg("-I")
            .arg(root.join("libs/libbreenix-libc/include"))
            .arg("-I")
            .arg(root.join("toolchain/include"))
            .arg("-o")
            .arg(&assembly)
            .arg(&source)
            .output()
            .unwrap();
        if !compile.status.success() || !compile.stderr.is_empty() {
            return Err(format!(
                "{target}: {}",
                String::from_utf8_lossy(&compile.stderr)
            ));
        }
        let assemble = Command::new(bcc.with_file_name("bas"))
            .args(["--target", target, "-o"])
            .arg(&object)
            .arg(&assembly)
            .output()
            .unwrap();
        if !assemble.status.success() || !assemble.stderr.is_empty() {
            return Err(String::from_utf8_lossy(&assemble.stderr).into_owned());
        }
        let bytes = fs::read(&object).unwrap();
        let elf = bobj::elf::Elf::parse(&bytes).unwrap();
        let symbols = elf.static_symbols().unwrap().unwrap();
        // IEEE 754 binary32/binary64/binary128 and the SDM double-extended
        // format. An empty tag uses a quiet NaN with no payload.
        for (name, size, expected) in [
            ("inf", 4, 0x7f80_0000),
            ("huge_f", 4, 0x7f80_0000),
            ("huge_d", 8, 0x7ff0_0000_0000_0000),
            ("huge_l", 16, long_bits),
            ("quiet_nan", 4, 0x7fc0_0000),
            ("promoted_inf", 8, 0x7ff0_0000_0000_0000),
            ("promoted_nan", 8, 0x7ff8_0000_0000_0000),
            ("payload", 4, 0x7fc0_0123),
        ] {
            let symbol = (1..symbols.len())
                .map(|index| symbols.get(index).unwrap())
                .find(|symbol| symbol.name == name.as_bytes())
                .ok_or_else(|| format!("missing {target} symbol {name}"))?;
            let data = elf.section_data(symbol.section.into()).unwrap().unwrap();
            let offset = symbol.value as usize;
            assert_eq!(
                &data[offset..offset + size],
                &expected.to_le_bytes()[..size],
                "{target} {name}"
            );
        }
    }
    Ok(())
}

#[test]
fn predicates_and_integer_builtins_execute() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let bcc = Path::new(env!("CARGO_BIN_EXE_bcc"));
    let scratch = std::env::temp_dir().join(format!("bcc-builtin-runtime-{}", std::process::id()));
    fs::create_dir(&scratch).unwrap();
    let result = execute_builtins(bcc, &root, &scratch);
    fs::remove_dir_all(scratch).unwrap();
    result.unwrap();
}

fn execute_builtins(bcc: &Path, root: &Path, scratch: &Path) -> Result<(), String> {
    let source = scratch.join("predicates.c");
    fs::write(
        &source,
        r#"
#include <alloca.h>
#include <byteswap.h>
#include <math.h>
#include <stddef.h>

static volatile float tiny = 0x1p-149f;
static volatile double nan_value = NAN, inf = INFINITY, negzero = -0.0;
static volatile long double wide_inf = HUGE_VALL;
static int reads;
static double read_nan(void) { ++reads; return nan_value; }
static int use_buffer(int first, unsigned char *p, int last) {
    p[0] = first;
    p[last] = 8;
    return p[0] + p[last];
}
struct aligned_value { _Alignas(32) unsigned char value; };
static int use_aligned(struct aligned_value v, unsigned char *p) {
    p[0] = v.value;
    return p[0];
}
struct pair { char first; int second; };
_Static_assert(offsetof(struct pair, second) == 4, "AMD64 int member alignment");

int main(void) {
    if (fpclassify(read_nan()) != FP_NAN || reads != 1 ||
        fpclassify(inf) != FP_INFINITE || fpclassify(tiny) != FP_SUBNORMAL ||
        fpclassify(negzero) != FP_ZERO || fpclassify(4.0) != FP_NORMAL)
        return 1;
    if (!isnan(nan_value) || !isinf(wide_inf) || isfinite(inf) || !isfinite(tiny) ||
        isnormal(tiny) || !isnormal(4.0L) || !signbit(negzero) || signbit(nan_value))
        return 2;
    if (isgreater(nan_value, 2.0) || isgreaterequal(2.0, nan_value) || isless(nan_value, 2.0) ||
        islessequal(nan_value, 2.0) || islessgreater(2.0, nan_value) || !isunordered(nan_value, 2.0) ||
        !isgreater(2, 1.0f) || !isgreaterequal(2.0, 2.0f) || !isless(1.0, 2.0L) ||
        !islessequal(negzero, 0.0) || !islessgreater(1.0, 2.0) || isunordered(1.0, 2.0))
        return 3;
    volatile unsigned long bits = 0x8000000000000003UL;
    volatile unsigned short short_word = 0x1234;
    volatile unsigned int int_word = 0x12345678U;
    volatile unsigned long long_word = 0x0123456789abcdefUL;
    if (bswap_16(short_word) != 0x3412 || bswap_32(int_word) != 0x78563412U ||
        bswap_64(long_word) != 0xefcdab8967452301UL ||
        __builtin_popcountl(bits++) != 3 || bits != 0x8000000000000004UL)
        return 4;
    volatile unsigned long count = 65;
    unsigned char *p = alloca(count);
    p[64] = 5;
    if (p[64] + use_buffer(13, alloca(count), 64) != 26 || p[64] != 5)
        return 5;
    struct aligned_value value = {7};
    if (use_aligned(value, alloca(count)) != 7 || p[64] != 5)
        return 6;
    return 0;
}
"#,
    )
    .unwrap();
    let assembly = scratch.join("predicates.s");
    let object = scratch.join("predicates.o");
    let executable = scratch.join("predicates.elf");
    let script = scratch.join("entry.ld");
    fs::write(
        &script,
        fs::read_to_string(root.join("userspace/programs/linker.ld"))
            .unwrap()
            .replace("ENTRY(_start)", "ENTRY(main)"),
    )
    .unwrap();
    check(
        Command::new(bcc)
            .args(["--target=x86_64", "-std=c17", "-fno-builtin", "-S", "-I"])
            .arg(root.join("libs/libbreenix-libc/include"))
            .arg("-I")
            .arg(root.join("toolchain/include"))
            .arg("-o")
            .arg(&assembly)
            .arg(source),
    )?;
    check(
        Command::new(bcc.with_file_name("bas"))
            .arg("-o")
            .arg(&object)
            .arg(assembly),
    )?;
    check(
        Command::new(bcc.with_file_name("bld"))
            .arg("-T")
            .arg(script)
            .arg("-o")
            .arg(&executable)
            .arg(object),
    )?;
    let mut python = if cfg!(target_os = "macos") {
        let mut command = Command::new("arch");
        command.args(["-x86_64", "/usr/bin/python3"]);
        command
    } else {
        Command::new("python3")
    };
    check(
        python
            .arg(root.join("toolchain/tests/bcc/return_42_exit_status.py"))
            .arg("--call-entry")
            .arg(executable),
    )
}

fn check(command: &mut Command) -> Result<(), String> {
    let output = command.output().unwrap();
    if output.status.success() && output.stderr.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{command:?}: {}\n{}{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}
