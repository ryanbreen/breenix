//! Commit-time serialization gate, independent of the syscall and GPU paths.
pub fn serialize_for_commit(readers: u64, count: &mut u64,
    serialize: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    if readers != 0 {
        serialize()?;
        *count += 1;
    }
    Ok(())
}
