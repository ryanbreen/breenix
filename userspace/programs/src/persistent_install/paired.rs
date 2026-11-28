//! Paired installer host checks exercise the existing replacement before the new path is added.
use std::path::Path;

#[path = "../../../../libs/libbreenix/src/crypto/sha256.rs"]
mod sha256;

pub(crate) fn digest(bytes: &[u8]) -> String {
    sha256::sha256(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn record_fixture(_: &Path) -> Result<String, String> {
    Ok("BREENIX-IMAGE-1\0".into())
}

pub(crate) fn have_fixture(_: &Path, _: &[u8]) -> Result<Vec<String>, String> {
    Ok(Vec::new())
}

pub(crate) fn verify_fixture(_: &Path, _: &[u8]) -> Result<String, String> {
    Err("the existing replacement has no inventory readback".into())
}

pub(crate) fn select_fixture(_: &Path, _: &str) -> Result<(), String> {
    Ok(())
}

pub(crate) fn apply_fixture(root: &Path, _: &str, _: &str, _: &[u8],
    _: &mut dyn FnMut() -> Result<(), String>) -> Result<String, String> {
    super::replace_usr(root, root).map(|_| String::new())
}
