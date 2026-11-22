#![forbid(unsafe_code)]

use std::path::Path;
use std::process::Command;

#[test]
fn objects_equal_c_bas() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/bas/parity.py");
    let status = Command::new("python3")
        .arg(script)
        .arg(env!("CARGO_BIN_EXE_bas"))
        .status()
        .expect("run bas parity harness");
    assert!(status.success(), "bas_parity::objects_equal_c_bas failed");
}
