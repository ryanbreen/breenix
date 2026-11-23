//! Resolve the BusyBox, shell and applet PATH for the libc and utilities suites.

use std::borrow::Cow;
use std::fs;
use std::io;
use std::path::Path;

/// 25.11d2 publishes this record on the mounted scratch fixture. Suite sequence
/// reset leaves the contents of a separate filesystem in place.
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
    pub fn load(record: &Path, suite: Suite) -> Result<Self, String> {
        let located = |error| format!("{}: {error}", record.display());
        // A dangling record symlink is a present but unreadable record.
        match fs::symlink_metadata(record) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Self::defaults(suite));
            }
            Err(error) => return Err(located(format!("cannot inspect record: {error}"))),
        }
        if !fs::metadata(record)
            .map_err(|error| located(format!("cannot read record: {error}")))?
            .is_file()
        {
            return Err(located("record is not a regular file".into()));
        }
        let text = fs::read_to_string(record)
            .map_err(|error| located(format!("cannot read record: {error}")))?;
        Self::parse(&text).map_err(located)
    }

    fn defaults(suite: Suite) -> Self {
        let (busybox, sh, path) = match suite {
            Suite::Libc => (
                "/usr/libexec/breenix/libc/bin/busybox",
                "/usr/libexec/breenix/libc/bin/sh",
                "/usr/libexec/breenix/libc/bin",
            ),
            Suite::Utilities => ("/bin/busybox", "/bin/sh", "/bin:/usr/bin"),
        };
        Self {
            busybox: busybox.into(),
            sh: sh.into(),
            path: path.into(),
            selected: false,
        }
    }

    fn parse(text: &str) -> Result<Self, String> {
        let (mut busybox, mut sh, mut path) = (None, None, None);
        for (index, line) in text.lines().enumerate() {
            let line_number = index + 1;
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| format!("line {line_number} needs key=value"))?;
            let slot = match key {
                "busybox" => &mut busybox,
                "sh" => &mut sh,
                "PATH" => &mut path,
                _ => return Err(format!("line {line_number} has unknown key {key:?}")),
            };
            if slot.replace(value).is_some() {
                return Err(format!("line {line_number} repeats {key}"));
            }
        }
        let busybox = busybox.ok_or("missing busybox")?;
        let sh = sh.ok_or("missing sh")?;
        let path = path.ok_or("missing PATH")?;
        check_path("busybox", busybox, false)?;
        check_path("sh", sh, false)?;
        for dir in path.split(':') {
            check_path("PATH", dir, true)?;
        }
        Ok(Self {
            busybox: busybox.to_owned().into(),
            sh: sh.to_owned().into(),
            path: path.to_owned().into(),
            selected: true,
        })
    }
}

fn check_path(field: &str, value: &str, directory: bool) -> Result<(), String> {
    let path = Path::new(value);
    if !path.is_absolute() || value.contains('\0') {
        return Err(format!("{field} needs an absolute path, got {value:?}"));
    }
    let error = |why| format!("{field} {value:?}: {why}");
    let metadata = fs::metadata(path).map_err(|why| error(why.to_string()))?;
    if directory {
        if !metadata.is_dir() {
            return Err(error("expected a directory".into()));
        }
        fs::read_dir(path).map_err(|why| error(why.to_string()))?;
    } else {
        if !metadata.is_file() {
            return Err(error("expected a regular file".into()));
        }
        fs::File::open(path).map_err(|why| error(why.to_string()))?;
    }
    #[cfg(target_os = "breenix")]
    {
        use libbreenix::fs::{access, R_OK, X_OK};
        access(value, R_OK | X_OK).map_err(|why| error(why.to_string()))?;
    }
    #[cfg(not(target_os = "breenix"))]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(error("missing execute permission".into()));
        }
    }
    Ok(())
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
