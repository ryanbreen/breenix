use std::fs;
use std::path::Path;
use std::process::Command;

fn checked(command: &mut Command) -> Vec<u8> {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}: {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

#[test]
fn spill_across_call() {
    // AMD64 psABI §3.2.1 (Fig. 3.4) and §3.2.3.
    let bcc = Path::new(env!("CARGO_BIN_EXE_bcc"));
    let toolchain = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let work = std::env::temp_dir().join(format!("bcc-regalloc-{}", std::process::id()));
    fs::create_dir_all(&work).unwrap();
    let source = work.join("probe.c");
    let assembly = work.join("probe.s");
    let clobber = work.join("clobber.s");
    let script = work.join("main.ld");
    let executable = work.join("probe");
    fs::write(
        &source,
        r"
extern long clobber(long, long, long, long, long, long);
long probe(long input, long *source) {
    long held = input * 3;
    long *address = source + 1;
    return held + (clobber(0, 1, 2, 3, 4, 5) + *address);
}
int main(void) {
    long values[] = {0, 8};
    return (int)probe(9, values);
}
",
    )
    .unwrap();
    let mut stub = String::from(
        "section .text\nglobal clobber\nclobber:\n\
         cmp r8, 4\n jne .bad\n cmp r9, 5\n jne .bad\n mov eax, 7\n jmp .clear\n\
         .bad:\n xor eax, eax\n .clear:\n",
    );
    for register in ["rcx", "rdx", "rsi", "rdi", "r8", "r9", "r10", "r11"] {
        stub.push_str(&format!("    xor {register}, {register}\n"));
    }
    for register in 0..16 {
        stub.push_str(&format!("    pxor xmm{register}, xmm{register}\n"));
    }
    stub.push_str("    ret\n");
    fs::write(&clobber, stub).unwrap();
    let body = checked(
        Command::new(bcc)
            .args(["--target=x86_64", "-O2", "-S"])
            .arg(&source),
    );
    fs::write(&assembly, &body).unwrap();
    for input in [&assembly, &clobber] {
        checked(
            Command::new(bcc.with_file_name("bas"))
                .arg("-o")
                .arg(input.with_extension("o"))
                .arg(input),
        );
    }
    let linker_script =
        fs::read_to_string(toolchain.join("../userspace/programs/linker.ld")).unwrap();
    fs::write(
        &script,
        linker_script.replace("ENTRY(_start)", "ENTRY(main)"),
    )
    .unwrap();
    checked(
        Command::new(bcc.with_file_name("bld"))
            .arg("-T")
            .arg(&script)
            .arg("--gc-sections")
            .arg("-o")
            .arg(&executable)
            .arg(assembly.with_extension("o"))
            .arg(clobber.with_extension("o")),
    );

    // Use the existing freestanding ELF entry runner, without a guest boot.
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
        .arg(&executable)
        .output()
        .unwrap();
    fs::remove_dir_all(&work).unwrap();
    assert_eq!(result.status.code(), Some(42), "{result:?}");

    let body = String::from_utf8(body).unwrap();
    let probe = body
        .split_once("\nprobe:\n")
        .unwrap()
        .1
        .split_once(".size probe,")
        .unwrap()
        .0;
    let (before, after) = probe.split_once("    call clobber@plt\n").unwrap();
    assert!(
        before.lines().any(|line| {
            let Some((spill, register)) = line
                .trim()
                .strip_prefix("mov qword [rbp - ")
                .and_then(|operands| operands.split_once("], "))
            else {
                return false;
            };
            ["r8", "r9", "r10", "r11"].contains(&register)
                && after
                    .lines()
                    .any(|line| line.trim() == format!("mov {register}, qword [rbp - {spill}]"))
        }),
        "expected an allocated caller-saved value to spill and reload around clobber:\n{probe}"
    );
}
