//! Drive the actual syscall arms with a controllable host backend and wait queue.
use std::{fs, path::PathBuf, process::Command};

const HANDLERS: &str = include_str!("../../../kernel/src/syscall/handlers.rs");
const FS: &str = include_str!("../../../kernel/src/syscall/fs.rs");
const FD: &str = include_str!("../../../kernel/src/ipc/fd.rs");

fn between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    source
        .split_once(start)
        .unwrap()
        .1
        .split_once(end)
        .unwrap()
        .0
}

#[test]
fn shared_description_transfers_are_atomic() {
    run_fixture("scalar", None);
}

#[test]
fn regular_file_vectors_are_atomic() {
    run_fixture("vector", Some("vector_"));
}

fn run_fixture(kind: &str, filter: Option<&str>) {
    let kernel = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../kernel/src");
    let temporary = std::env::temp_dir().join(format!(
        "breenix-shared-offset-{}-{kind}",
        std::process::id()
    ));
    fs::create_dir(&temporary).unwrap();

    let variant = FD
        .lines()
        .find(|line| line.trim_start().starts_with("RegularFile(Arc<"))
        .unwrap();
    let read = between(HANDLERS, "pub fn sys_read(", "FdKind::Directory(_) =>");
    let read = read
        .split_once("FdKind::RegularFile(file_ref) => {")
        .unwrap()
        .1;
    let write = between(
        HANDLERS,
        "WriteOperation::RegularFile {\n            file,",
        "\n    };\n    if matches!(result",
    );
    let (bindings, write) = write.split_once("} => {").unwrap();
    let write = write.strip_suffix("\n        }").unwrap();
    let append = if bindings.contains("description") {
        "let description = entry.description.clone();"
    } else {
        "let append = entry.description.status_flags() & crate::ipc::fd::status_flags::O_APPEND != 0;"
    };
    let constructor = between(FS, "fn prepare_open_descriptor(", "/// A descriptor slot")
        .split_once("} else {")
        .unwrap()
        .1
        .split_once("\n    };")
        .unwrap()
        .0;
    let gate = if kernel.join("ipc/sleep_mutex.rs").exists() {
        format!(
            "#[path = {:?}] mod sleep_mutex;",
            kernel.join("ipc/sleep_mutex.rs")
        )
    } else {
        String::new()
    };

    let fixture = include_str!("fixtures/shared_offset.rs")
        .replace("// SLEEP_MUTEX", &gate)
        .replace(
            "IOVEC_RANGE_PATH",
            &format!("{:?}", kernel.join("syscall/iovec_range.rs")),
        )
        .replace(
            "IOVEC_PATH",
            &format!("{:?}", kernel.join("syscall/iovec.rs")),
        )
        .replace(
            "REGULAR_READ_PATH",
            &format!("{:?}", kernel.join("syscall/regular_read.rs")),
        )
        .replace(
            "// REGULAR_FILE",
            &format!(
                "pub struct RegularFile {{{}}}",
                between(FD, "pub struct RegularFile {", "\n}")
            ),
        )
        .replace("// REGULAR_VARIANT", variant)
        .replace("// CONSTRUCTOR", constructor)
        .replace("// READ_ARM", read)
        .replace("// WRITE_BINDING", append)
        .replace("// WRITE_ARM", write)
        .replace(
            "// POSITIONED_HANDLERS",
            &format!(
                "pub(super) fn read_positional_file{}\npub(super) fn write_positional_file{}",
                between(
                    HANDLERS,
                    "pub(super) fn read_positional_file",
                    "/// pwrite64"
                ),
                HANDLERS
                    .split_once("pub(super) fn write_positional_file")
                    .unwrap()
                    .1,
            ),
        )
        .replace(
            "// LSEEK",
            &format!(
                "pub fn sys_lseek{}",
                between(FS, "pub fn sys_lseek", "/// sys_fstat")
            ),
        );
    let source = temporary.join("fixture.rs");
    let binary = temporary.join("fixture");
    fs::write(&source, fixture).unwrap();
    let compiled = Command::new("rustc")
        .args(["--edition=2021", "--test", "-Dwarnings"])
        .arg(&source)
        .arg("-o")
        .arg(&binary)
        .status()
        .unwrap();
    assert!(compiled.success(), "could not compile {}", source.display());
    let mut command = Command::new(&binary);
    match filter {
        Some(filter) => command.arg(filter),
        None => command.args(["--skip", "vector_"]),
    };
    let status = command.status().unwrap();
    fs::remove_dir_all(&temporary).unwrap();
    assert!(status.success(), "fixed interleavings failed");
}
