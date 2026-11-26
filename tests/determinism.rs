//! Opt-in Mac twin comparison: rustc --test tests/determinism.rs -o /tmp/determinism.
use std::fs;
use std::process::Command;

#[test]
fn two_directories_same_bytes() {
    let fixture = std::env::var_os("BREENIX_K6_FIXTURE")
        .expect("set BREENIX_K6_FIXTURE for the opt-in Mac twin comparison");
    let root = std::env::current_dir().unwrap();
    let work = std::env::temp_dir().join(format!("breenix-selfhost-{}", std::process::id()));
    fs::create_dir(&work).unwrap();
    let runner = work.join("selfhost-mac");
    assert!(Command::new("rustc")
        .current_dir(&root)
        .args(["--edition=2021", "scripts/rust-host/selfhost-mac.rs", "-o"])
        .arg(&runner)
        .status()
        .unwrap()
        .success());
    assert!(
        Command::new(&runner)
            .arg(&root)
            .arg(&fixture)
            .arg(&work)
            .status()
            .unwrap()
            .success(),
        "Mac twin build failed; evidence retained in {}",
        work.display()
    );
    for name in ["bas", "bld", "bar", "bnm", "bmake", "bcc", "libbobj.rlib"] {
        let left = work
            .join("one/run/target/x86_64-breenix/selfhost")
            .join(name);
        let right = work
            .join("two/run/target/x86_64-breenix/selfhost")
            .join(name);
        assert_eq!(
            fs::read(&left).unwrap(),
            fs::read(&right).unwrap(),
            "different output: {name}; evidence retained in {}",
            work.display()
        );
    }
    fs::remove_dir_all(work).unwrap();
}
