use std::path::Path;
use std::process::Command;

#[test]
fn hello_translation_unit_types() {
    let runner = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/cconf/run.py");
    let failures: Vec<_> = ["x86_64", "aarch64"]
        .into_iter()
        .filter_map(|target| {
            let result = Command::new("python3")
                .arg(&runner)
                .args([
                    "--cc",
                    env!("CARGO_BIN_EXE_bcc"),
                    "--mode",
                    "types",
                    "--stage",
                    "b1",
                    "--types-flag",
                    "--dump-types",
                    "--target",
                    target,
                    "--cc-arg",
                    &format!("--target={target}"),
                ])
                .output()
                .expect("run cconf hello-types");
            let stdout = String::from_utf8_lossy(&result.stdout);
            if result.status.success() && stdout.lines().any(|line| line == "cconf::hello-types PASS") {
                return None;
            }
            Some(format!(
                "{target} ({}):\n{stdout}{}",
                result.status,
                String::from_utf8_lossy(&result.stderr)
            ))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
