//! Temporary-directory driver for persistent_migrate_test.py.
#[path = "../userspace/programs/src/persistent_migrate/core.rs"]
mod migrate;
#[path = "../userspace/programs/src/persistent_migrate/finish.rs"]
mod finish;
#[path = "../libs/libbreenix/src/layout.rs"]
pub mod layout;
#[path = "../libs/libbreenix/src/crypto/sha256.rs"]
pub mod sha256;

use migrate::{Mounts, Paths};
use std::{cell::Cell, fs, io, os::unix::fs::PermissionsExt, path::PathBuf, process::ExitCode};

struct Fixture<'a> {
    root: PathBuf,
    switch: usize,
    mismatch: Option<String>,
    latch_run_at_snapshot: Option<&'a Cell<bool>>,
}

impl Mounts for Fixture<'_> {
    fn snapshot(&mut self, name: &str) -> io::Result<()> {
        assert_eq!(name, "gen-fixture");
        assert!(self.root.join("default").read_dir()?.next().is_none());
        fs::create_dir(self.root.join("generation"))?;
        if let Some(run_read_only) = self.latch_run_at_snapshot {
            run_read_only.set(true);
            return Err(io::ErrorKind::ReadOnlyFilesystem.into());
        }
        Ok(())
    }

    fn switch_system(&mut self, root: Option<&str>, read_only: bool) -> io::Result<PathBuf> {
        let expected = [(Some("gen-fixture"), false), (None, true), (Some("gen-fixture"), true)];
        assert_eq!((root, read_only), expected[self.switch]);
        self.switch += 1;
        let path = self.root.join(if root.is_some() { "generation" } else { "default" });
        if self.switch == expected.len() {
            match self.mismatch.as_deref() {
                Some("mismatch") => {
                    let file = path.join("bin/tool");
                    let metadata = file.metadata()?;
                    fs::write(&file, b"wrong")?;
                    fs::File::open(&file)?.set_modified(metadata.modified()?)?;
                }
                Some("file-mode") => fs::set_permissions(
                    path.join("bin/tool"), fs::Permissions::from_mode(0o0755),
                )?,
                Some("directory-mode") => fs::set_permissions(
                    path.join("root"), fs::Permissions::from_mode(0o1700),
                )?,
                None => {}
                Some(other) => panic!("unknown fixture mutation {other}"),
            }
        }
        Ok(path)
    }

    fn home_read_only(&mut self) -> io::Result<()> {
        assert!(self.root.join("home/profile").is_file());
        Ok(())
    }
}

fn main() -> ExitCode {
    let root = PathBuf::from(std::env::args_os().nth(1).unwrap());
    let paths = Paths {
        source_system: &root.join("ext2-system"),
        source_home: &root.join("ext2-home"),
        system: &root.join("default"),
        home: &root.join("home"),
        evidence: &root.join("evidence"),
    };
    let mode = std::env::args().nth(2);
    let latch_run = mode.as_deref() == Some("--vm-poweroff-run-read-only");
    let vm_poweroff = latch_run || mode.as_deref() == Some("--vm-poweroff");
    let run_read_only = Cell::new(false);
    let mut mounts = Fixture {
        root: root.clone(),
        switch: 0,
        mismatch: mode.filter(|_| !vm_poweroff),
        latch_run_at_snapshot: latch_run.then_some(&run_read_only),
    };
    let mut migrate = || migrate::run(&paths, "fixture", &mut mounts, &mut io::stdout());
    if vm_poweroff {
        let nonce_path = root.join("run/breenix/shutdown-nonce");
        let prepare_shutdown = || -> io::Result<String> {
            if run_read_only.get() {
                return Err(io::ErrorKind::ReadOnlyFilesystem.into());
            }
            let nonce = "a".repeat(64);
            fs::create_dir_all(nonce_path.parent().unwrap())?;
            fs::write(&nonce_path, format!("{nonce}\n"))?;
            Ok(nonce)
        };
        let request_poweroff = |nonce: String| {
            assert_eq!(fs::read_to_string(&nonce_path)?, format!("{nonce}\n"));
            fs::write(root.join("poweroff-requested"), nonce)
        };
        return finish::power_off_after(migrate(), || request_poweroff(prepare_shutdown()?));
    }
    match migrate() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
