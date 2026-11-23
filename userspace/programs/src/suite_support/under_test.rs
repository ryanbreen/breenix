//! Resolve the BusyBox, shell and applet PATH for the libc and utilities suites.

use std::borrow::Cow;
use std::path::Path;

pub const RECORD: &str = "/mnt/breenix-toolchain/busybox-under-test";

#[derive(Clone, Copy)]
pub enum Suite {
    Libc,
    Utilities,
}

#[derive(Debug)]
pub struct Paths {
    pub busybox: Cow<'static, str>,
    pub sh: Cow<'static, str>,
    pub path: Cow<'static, str>,
    pub selected: bool,
}

impl Paths {
    pub fn load(_record: &Path, suite: Suite) -> Result<Self, String> {
        let (busybox, sh, path) = match suite {
            Suite::Libc => (
                "/usr/libexec/breenix/libc/bin/busybox",
                "/usr/libexec/breenix/libc/bin/sh",
                "/usr/libexec/breenix/libc/bin",
            ),
            Suite::Utilities => ("/bin/busybox", "/bin/sh", "/bin:/usr/bin"),
        };
        Ok(Self {
            busybox: busybox.into(),
            sh: sh.into(),
            path: path.into(),
            selected: false,
        })
    }
}

#[cfg(test)]
mod under_test {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::path::PathBuf;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("under-test-{}-{name}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn record(&self) -> PathBuf {
            self.0.join("busybox-under-test")
        }

        fn executable(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, b"fixture").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        }

        fn write_record(&self, busybox: &str, sh: &str, path: &str) {
            fs::write(
                self.record(),
                format!("busybox={busybox}\nsh={sh}\nPATH={path}\n"),
            )
            .unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn absent_record_keeps_defaults() {
        let fixture = Fixture::new("absent");
        let libc = Paths::load(&fixture.record(), Suite::Libc).unwrap();
        assert!(!libc.selected);
        assert_eq!(libc.busybox, "/usr/libexec/breenix/libc/bin/busybox");
        assert_eq!(libc.sh, "/usr/libexec/breenix/libc/bin/sh");
        assert_eq!(libc.path, "/usr/libexec/breenix/libc/bin");

        let utilities = Paths::load(&fixture.record(), Suite::Utilities).unwrap();
        assert!(!utilities.selected);
        assert_eq!(utilities.busybox, "/bin/busybox");
        assert_eq!(utilities.sh, "/bin/sh");
        assert_eq!(utilities.path, "/bin:/usr/bin");
    }

    #[test]
    fn record_selects_paths() {
        let fixture = Fixture::new("selected");
        let busybox = fixture.executable("busybox");
        let sh = fixture.0.join("sh");
        symlink(&busybox, &sh).unwrap();
        let extra = fixture.0.join("extra applets");
        fs::create_dir(&extra).unwrap();
        let path = format!("{}:{}", fixture.0.display(), extra.display());
        fixture.write_record(busybox.to_str().unwrap(), sh.to_str().unwrap(), &path);

        for suite in [Suite::Libc, Suite::Utilities] {
            let resolved = Paths::load(&fixture.record(), suite).unwrap();
            assert!(resolved.selected);
            assert_eq!(resolved.busybox, busybox.to_str().unwrap());
            assert_eq!(resolved.sh, sh.to_str().unwrap());
            assert_eq!(resolved.path, path);
        }
    }

    #[test]
    fn malformed_record_is_located_failure() {
        let fixture = Fixture::new("invalid");
        let busybox = fixture.executable("busybox");
        let sh = fixture.executable("sh");
        let unreadable = fixture.executable("unreadable");
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o100)).unwrap();
        let missing = fixture.0.join("missing");
        let (busybox, sh, unreadable, missing, bin) = (
            busybox.to_str().unwrap(),
            sh.to_str().unwrap(),
            unreadable.to_str().unwrap(),
            missing.to_str().unwrap(),
            fixture.0.to_str().unwrap(),
        );
        let record = fixture.record();
        let check_error = |field: &str| {
            for suite in [Suite::Libc, Suite::Utilities] {
                let error = Paths::load(&record, suite).unwrap_err();
                assert!(
                    error.starts_with(&format!("{}: ", record.display())),
                    "{error}"
                );
                assert!(error.contains(field), "{error}");
            }
        };
        for (busybox, sh, path, field) in [
            ("busybox", sh, bin, "busybox"),
            (busybox, missing, bin, "sh"),
            (unreadable, sh, bin, "busybox"),
            (busybox, sh, "relative", "PATH"),
            (busybox, sh, missing, "PATH"),
            (busybox, sh, &format!("{bin}:"), "PATH"),
        ] {
            fixture.write_record(busybox, sh, path);
            check_error(field);
        }

        fs::write(&record, format!("busybox={busybox}\nPATH={bin}\n")).unwrap();
        check_error("sh");
        fs::remove_file(&record).unwrap();
        symlink(missing, &record).unwrap();
        check_error("read");
    }
}
