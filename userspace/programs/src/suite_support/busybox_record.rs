//! Publish the applet paths after the newly linked BusyBox has reported its applets.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub(super) fn publish(record: &Path, binary: &Path, applets: &[&str]) -> Result<(), String> {
    let location = |error| format!("{}: {error}", record.display());
    let install = || -> std::io::Result<()> {
        let metadata = fs::metadata(binary)?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return Err(std::io::Error::other("linked BusyBox must be executable"));
        }
        let bin = binary.parent().ok_or_else(|| std::io::Error::other("missing build directory"))?
            .join("bin");
        if !bin.is_absolute() || bin.to_str().is_none_or(|path| path.contains([':', '\n', '\r']))
            || !applets.contains(&"sh") || applets.iter().any(|name| {
                name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"[-_".contains(&byte))
            })
        {
            return Err(std::io::Error::other("invalid applet installation paths"));
        }
        fs::create_dir(&bin)?;
        let busybox = bin.join("busybox");
        let sh = bin.join("sh");
        fs::hard_link(binary, &busybox)?;
        for applet in applets {
            fs::hard_link(binary, bin.join(applet))?;
        }

        let staging = bin.join(".under-test");
        let mut file = OpenOptions::new().write(true).create_new(true).open(&staging)?;
        writeln!(file, "busybox={}\nsh={}\nPATH={}", busybox.display(), sh.display(), bin.display())?;
        file.sync_all()?;
        fs::rename(staging, record)?;
        Ok(())
    };
    install().map_err(location)
}
