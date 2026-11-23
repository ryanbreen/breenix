use std::fs;
use std::path::Path;
use std::process::Command;

// N1570 6.7.9p17–22, 6.5.2.5p4–5, and 6.7.2.1p18.
#[test]
fn designated_init_last_wins() {
    let work = std::env::temp_dir().join(format!("breenix-bcc-c11-{}", std::process::id()));
    fs::create_dir(&work).unwrap();
    struct Scratch(std::path::PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let scratch = Scratch(work);
    let work = &scratch.0;
    let source = work.join("c11.c");
    let assembly = work.join("c11.s");
    let object = work.join("c11.o");
    let executable = work.join("c11.elf");
    fs::write(
        &source,
        r"
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
",
    )
    .unwrap();

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

fn checked(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
