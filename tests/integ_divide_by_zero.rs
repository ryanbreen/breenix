use std::process::Command;

#[test]
fn divide_by_zero() {
    // Build & run kernel requesting just this test
    let out = Command::new("cargo")
        .args([
            "run", "-p", "xtask", "--", "build-and-run",
            "--features", "testing", 
            "--timeout", "15"
        ])
        .env("BREENIX_TEST", "tests=divide_by_zero")
        .output()
        .expect("run kernel tests");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    
    if !out.status.success() {
        eprintln!("STDOUT:\n{}", stdout);
        eprintln!("STDERR:\n{}", stderr);
        panic!("kernel run failed with exit code: {:?}", out.status.code());
    }
    
    assert!(
        stdout.contains("TEST_MARKER: DIV0_OK"),
        "marker not found in:\nSTDOUT:\n{}\nSTDERR:\n{}", stdout, stderr
    );
}