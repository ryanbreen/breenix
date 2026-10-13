//! Descriptor selection shared by descriptor-relative pathname syscalls.
pub(crate) const AT_FDCWD: i32 = -100;
pub(crate) fn directory_start<T>(
    dirfd: i32,
    absolute: bool,
    lookup: impl FnOnce(i32) -> Result<T, u64>,
) -> Result<Option<T>, u64> {
    if absolute || dirfd == AT_FDCWD { Ok(None) } else { lookup(dirfd).map(Some) }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absolute_and_cwd_names_ignore_invalid_descriptors() {
        for (fd, absolute) in [(-1, true), (42, true), (AT_FDCWD, false)] {
            assert_eq!(directory_start::<u32>(fd, absolute, |_| panic!("unexpected lookup")), Ok(None));
        }
    }
    #[test]
    fn relative_names_require_a_held_directory() {
        assert_eq!(directory_start(42, false, |fd| Ok(fd)), Ok(Some(42)));
        for error in [9, 20, 13] { assert_eq!(directory_start::<u32>(42, false, |_| Err(error)), Err(error)); }
    }
}
