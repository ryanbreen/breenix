//! The boot-target file (`kernel/src/boot/target.rs`): the `suite <id>`,
//! `suite <id> case <category>/<case>`, `probe` and `program <path> [<word>]` lines
//! the runners write, and
//! the lines a boot refuses.

extern crate alloc;

#[path = "../kernel/src/boot/target.rs"]
mod target;

/// The mounted root as `target::read` sees it: one optional boot-target file.
mod fs {
    pub mod ext2 {
        use std::sync::Mutex;

        pub static FILE: Mutex<Option<Vec<u8>>> = Mutex::new(None);

        pub struct Inode(Vec<u8>);

        impl Inode {
            pub fn is_dir(&self) -> bool {
                false
            }

            pub fn size(&self) -> u64 {
                self.0.len() as u64
            }
        }

        pub struct Root;

        impl Root {
            pub fn resolve_path(&self, path: &str) -> Result<u32, ()> {
                match FILE.lock().unwrap().as_ref() {
                    Some(_) if path == crate::target::PATH => Ok(2),
                    _ => Err(()),
                }
            }

            pub fn read_inode(&self, _inode: u32) -> Result<Inode, ()> {
                FILE.lock().unwrap().clone().map(Inode).ok_or(())
            }

            pub fn read_file_content_coherent(&self, _inode: u32, inode: &Inode) -> Result<Vec<u8>, ()> {
                Ok(inode.0.clone())
            }
        }

        pub fn root_fs_read() -> Result<Option<Root>, &'static str> {
            Ok(Some(Root))
        }
    }
}

use target::{parse, Target};

fn suite(text: &str) -> (String, Option<String>) {
    match parse(text) {
        Ok(Target::Suite(suite)) => (suite.id, suite.case),
        Ok(Target::Probe | Target::Default | Target::Shell | Target::Console { .. } | Target::Program { .. }) => {
            panic!("{text:?} parsed as another mode")
        }
        Err(why) => panic!("{text:?} was refused: {why}"),
    }
}

#[test]
fn hostkeys_declaration_is_preserved_and_refuses_injection() {
    for selection in ["suite console", "suite console case keys/alt-fn"] {
        let Target::Suite(suite) = parse(&format!("{selection} hostkeys prlctl;missing=left,right\n")).unwrap() else { panic!("suite expected") };
        assert_eq!(suite.hostkeys.as_deref(), Some("prlctl;missing=left,right"));
    }
    for text in ["suite console hostkeys ", "suite console hostkeys qmp extra", "probe hostkeys qmp", "suite console hostkeys qmp hostkeys vnc"] {
        assert!(parse(text).is_err(), "{text}");
    }
}

#[test]
fn suite_and_probe_lines_parse() {
    assert_eq!(suite("suite smoke\n"), ("smoke".into(), None));
    assert_eq!(suite("suite files-io\r\n"), ("files-io".into(), None));
    assert_eq!(
        suite("suite smoke case process/getpid\n"),
        ("smoke".into(), Some("process/getpid".into()))
    );
    // Unknown names stay intact so the suite can list its valid ids.
    assert_eq!(
        suite("suite smoke case No_Such.case/x-1"),
        ("smoke".into(), Some("No_Such.case/x-1".into()))
    );
    assert!(matches!(parse("probe\n"), Ok(Target::Probe)));
    assert!(matches!(parse("default\n"), Ok(Target::Default)));
    assert!(matches!(parse("shell\n"), Ok(Target::Shell)));
    assert!(matches!(parse("console\n"), Ok(Target::Console { .. })));
}

#[test]
fn program_lines_carry_an_absolute_path_and_one_word() {
    let Ok(Target::Program { path, arg }) = parse("program /bin/usb_pointer_check\n") else { panic!("program expected") };
    assert_eq!((path.as_str(), arg), ("/bin/usb_pointer_check", None));
    let Ok(Target::Program { path, arg }) = parse("program /bin/usb_pointer_check replay\n") else { panic!("program expected") };
    assert_eq!((path.as_str(), arg.as_deref()), ("/bin/usb_pointer_check", Some("replay")));
    for text in [
        "program\n",
        "program bin/usb_pointer_check\n",
        "program /\n",
        "program /bin/a;b\n",
        "program /bin/a Replay\n",
        "program /bin/a host extra\n",
        "program  /bin/a\n",
    ] {
        assert!(parse(text).is_err(), "{text:?} was accepted");
    }
}

#[test]
fn malformed_case_selections_are_refused() {
    for text in [
        "suite smoke case\n",
        "suite smoke case \n",
        "suite smoke case process/getpid extra\n",
        "suite smoke case process/getpid,memory/mmap\n",
        "suite smoke kase process/getpid\n",
        "suite smoke  case process/getpid\n",
        "suite smoke case  process/getpid\n",
        "suite\tsmoke\n",
        "suite smoke\tcase process/getpid\n",
        "suite  smoke\n",
        "suite Smoke\n",
        "suite smoke\nsuite memory\n",
        "suite\n",
        "",
    ] {
        assert!(parse(text).is_err(), "{text:?} was accepted");
    }
}

#[test]
fn case_tokens_are_checked_when_built_from_fw_cfg() {
    use target::SuiteTarget;
    assert!(SuiteTarget::new("smoke".into(), Some("process/getpid".into())).is_ok());
    for case in ["", "process/getpid,memory/mmap", "process getpid", "process/getpid\n"] {
        assert!(SuiteTarget::new("smoke".into(), Some(case.into())).is_err(), "{case:?} was accepted");
    }
    assert!(SuiteTarget::new("not a suite".into(), None).is_err());
    assert_eq!(target::suite_path("smoke"), "/sbin/suite-smoke");
}

#[test]
fn read_parses_the_file_on_the_root() {
    *fs::ext2::FILE.lock().unwrap() = None;
    assert!(matches!(target::read(), Ok(None)));
    *fs::ext2::FILE.lock().unwrap() = Some(b"suite memory case anonymous/page-aligned\n".to_vec());
    match target::read() {
        Ok(Some(Target::Suite(suite))) => {
            assert_eq!(suite.id, "memory");
            assert_eq!(suite.case.as_deref(), Some("anonymous/page-aligned"));
        }
        _ => panic!("the boot-target file was not read as a one-case suite"),
    }
    *fs::ext2::FILE.lock().unwrap() = Some(vec![b'x'; 300]);
    assert!(target::read().is_err());
    *fs::ext2::FILE.lock().unwrap() = None;
}

#[test]
fn check_elf_refuses_a_non_elf() {
    assert!(target::check_elf(b"not an elf").is_err());
}

#[test]
fn console_idle_threshold_is_bounded_and_keeps_the_console_target() {
    assert!(matches!(parse("console\n"), Ok(Target::Console { idle_minutes: 30, replug: false })));
    assert!(matches!(parse("console idle=2\n"), Ok(Target::Console { idle_minutes: 2, replug: false })));
    assert!(matches!(parse("console idle=240\n"), Ok(Target::Console { idle_minutes: 240, replug: false })));
    for text in ["console idle=0", "console idle=241", "console idle=-1", "console idle=+2",
                 "console idle=", "console  idle=2", "console idle=2 extra", "shell idle=2"] {
        assert!(parse(text).is_err(), "{text:?} was accepted");
    }
}

#[test]
fn only_an_explicit_measured_console_target_requests_replug() {
    assert!(matches!(parse("console replug\n"),
        Ok(Target::Console { idle_minutes: 30, replug: true })));
    for text in ["console idle=2 replug", "console replug idle=2", "shell replug", "console  replug"] {
        assert!(parse(text).is_err(), "{text:?} was accepted");
    }
}
