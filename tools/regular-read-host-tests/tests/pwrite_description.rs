//! Drive the production pwrite64 arm with a close of its descriptor while the
//! write is pending (E3-3, docs/plans/e3-page-cache.md 10.3).
use std::{fs, process::Command};

const HANDLERS: &str = include_str!("../../../kernel/src/syscall/handlers.rs");

fn between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    source.split_once(start).unwrap().1.split_once(end).unwrap().0
}

#[test]
fn a_close_during_pwrite_keeps_write_access_until_the_write_returns() {
    let arm = format!(
        "pub fn sys_pwrite64{}\npub(super) fn write_positional_file{}",
        between(HANDLERS, "pub fn sys_pwrite64", "/// Write one range without using O_APPEND"),
        HANDLERS.split_once("pub(super) fn write_positional_file").unwrap().1,
    );
    let fixture = include_str!("fixtures/pwrite_description.rs").replace("// PWRITE64", &arm);
    let temporary = std::env::temp_dir().join(format!("breenix-pwrite-description-{}", std::process::id()));
    fs::create_dir(&temporary).unwrap();
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
    let status = Command::new(&binary).status().unwrap();
    fs::remove_dir_all(&temporary).unwrap();
    assert!(status.success(), "the fixed interleaving failed");
}
