//! Deterministic empty formatter input for writer checks.
pub fn image() -> std::vec::Vec<u8> {
    use std::{fs, process::Command, sync::OnceLock};
    static IMAGE: OnceLock<std::vec::Vec<u8>> = OnceLock::new();
    IMAGE
        .get_or_init(|| {
            let path = std::env::temp_dir()
                .join(std::format!("cowfs-files-empty-{}.img", std::process::id()));
            let output = Command::new(env!("CARGO_BIN_FILE_COWFS_cowfs-mkfs"))
                .args([
                    "--size",
                    "67108864",
                    "--time",
                    "0",
                    "--uuid",
                    "00112233445566778899aabbccddeeff",
                    "--hash-key",
                    "000102030405060708090a0b0c0d0e0f",
                ])
                .arg(&path)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            let bytes = fs::read(&path).unwrap();
            fs::remove_file(path).unwrap();
            bytes
        })
        .clone()
}

pub fn check(bytes: &[u8], tag: &str) {
    use std::{fs, process::Command};
    let path = std::env::temp_dir().join(std::format!(
        "cowfs-files-check-{}-{tag}.img",
        std::process::id()
    ));
    fs::write(&path, bytes).unwrap();
    let output = Command::new(env!("CARGO_BIN_FILE_COWFS_cowfs-check"))
        .arg("--full")
        .arg(&path)
        .output()
        .unwrap();
    fs::remove_file(path).unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
}
