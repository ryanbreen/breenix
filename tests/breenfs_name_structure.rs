use std::{fs, path::PathBuf, process::Command};

#[test]
fn tracked_paths_and_contents_use_breenfs() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let tracked = Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(&root)
        .output()
        .expect("list tracked paths");
    assert!(tracked.status.success(), "git ls-files failed");

    // Construct the retired spelling without introducing it into the source scan.
    let retired = [b"co".as_slice(), b"wfs".as_slice()].concat();
    let names_retired = |bytes: &[u8]| {
        bytes
            .windows(retired.len())
            .any(|word| word.eq_ignore_ascii_case(&retired))
    };
    // This scan exempts no tracked path or contents, including its own source.
    let mut violations = Vec::new();
    let mut count = 0;
    for path in tracked
        .stdout
        .split(|byte| *byte == 0)
        .filter(|p| !p.is_empty())
    {
        count += 1;
        let path = std::str::from_utf8(path).expect("tracked path is UTF-8");
        if names_retired(path.as_bytes()) {
            violations.push(format!("path: {path}"));
        }
        let full = root.join(path);
        let metadata =
            fs::symlink_metadata(&full).unwrap_or_else(|error| panic!("inspect {path}: {error}"));
        let bytes = if metadata.file_type().is_symlink() {
            fs::read_link(&full)
                .expect("read tracked symlink")
                .into_os_string()
                .into_encoded_bytes()
        } else {
            fs::read(&full).unwrap_or_else(|error| panic!("read {path}: {error}"))
        };
        if names_retired(&bytes) {
            violations.push(format!("contents: {path}"));
        }
    }
    assert!(count > 0, "tracked file list is empty");
    assert!(
        violations.is_empty(),
        "retired filesystem name remains:\n{}",
        violations.join("\n")
    );
}
