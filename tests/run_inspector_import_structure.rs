use std::{fs, path::Path};

#[test]
fn six_gates_source_and_call_the_post_verdict_importer() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for name in [
        "run-aarch64-boot-test-strict.sh",
        "run-aarch64-prod-profile-boot-test.sh",
        "run-aarch64-testing-profile-boot-test.sh",
        "run-x86-boot-tests.sh",
        "run-x86-gate.sh",
        "run-x86-prod-profile-boot-test.sh",
    ] {
        let text = fs::read_to_string(root.join("docker/qemu").join(name)).unwrap();
        let code: Vec<_> = text.lines().map(str::trim)
            .filter(|line| !line.starts_with('#')).collect();
        assert!(code.iter().any(|line| line.starts_with("source ")
            && line.contains("/lib/run-inspector-import.sh")), "{name}: missing importer source");
        assert!(code.iter().any(|line| line.starts_with("breenix_runs_import_nonfatal ")),
            "{name}: missing post-verdict import call");
    }
}
