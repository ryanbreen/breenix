//! Directory-fd selection before FIFO creation pathname resolution.
pub(crate) const AT_FDCWD: i32 = -100;
/// Absolute names ignore dirfd, including a descriptor that is not valid.
/// Relative AT_FDCWD names use cwd; other relative names must resolve dirfd.
pub(crate) fn directory_start<T>(
    dirfd: i32,
    absolute: bool,
    lookup: impl FnOnce(i32) -> Result<T, u64>,
) -> Result<Option<T>, u64> {
    if absolute || dirfd == AT_FDCWD {
        Ok(None)
    } else {
        lookup(dirfd).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::directory_start;

    #[test]
    fn absolute_names_and_cwd_do_not_inspect_dirfd() {
        for (fd, absolute) in [(-1, true), (42, true), (-100, false)] {
            assert_eq!(
                directory_start::<u32>(fd, absolute, |_| panic!("unused dirfd lookup")),
                Ok(None)
            );
        }
    }

    #[test]
    fn relative_names_resolve_the_descriptor_and_preserve_errors() {
        assert_eq!(
            directory_start(42, false, |fd| {
                assert_eq!(fd, 42);
                Ok(17)
            }),
            Ok(Some(17))
        );
        for error in [9, 20, 13] {
            assert_eq!(
                directory_start::<u32>(42, false, |_| Err(error)),
                Err(error)
            );
        }
    }
}
