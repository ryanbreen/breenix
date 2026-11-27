use super::*;
use std::io::Read;

const OLD: &[u8] = b"old destination";
const NEW: &[u8] = b"abc";
// SHA-256 of "abc", from an independent expectation.
const DIGEST: [u8; 32] = [
    0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22, 0x23,
    0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00, 0x15, 0xad,
];

struct Fixture {
    dest: Vec<u8>,
    temp: Option<Vec<u8>>,
    stdin: &'static [u8],
    /// A file an SFTP upload left in the staging directory, and how much of
    /// it has been read.
    staged: Option<Vec<u8>>,
    staged_read: usize,
    fault: Option<&'static str>,
    calls: Vec<&'static str>,
}

impl Fixture {
    fn new(stdin: &'static [u8], fault: Option<&'static str>) -> Self {
        Self {
            dest: OLD.to_vec(),
            temp: None,
            stdin,
            staged: None,
            staged_read: 0,
            fault,
            calls: Vec::new(),
        }
    }

    fn staged(bytes: &[u8], fault: Option<&'static str>) -> Self {
        Self {
            staged: Some(bytes.to_vec()),
            ..Self::new(b"", fault)
        }
    }

    fn step(&mut self, name: &'static str) -> io::Result<()> {
        self.calls.push(name);
        if self.fault == Some(name) {
            let kind = match name {
                "stdin" => io::ErrorKind::Interrupted,
                "progress" => io::ErrorKind::BrokenPipe,
                _ => io::ErrorKind::PermissionDenied,
            };
            return Err(io::Error::new(kind, format!("injected {name}")));
        }
        Ok(())
    }
}

impl PublishIo for Fixture {
    type File = ();

    fn open_dir(&mut self) -> io::Result<()> {
        self.step("open directory")
    }

    fn create_temp(&mut self) -> io::Result<()> {
        self.step("open temporary file")?;
        self.temp = Some(Vec::new());
        Ok(())
    }

    fn read_stdin(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.step("stdin")?;
        self.stdin.read(buf)
    }

    fn write_temp(&mut self, _: &mut (), buf: &[u8]) -> io::Result<usize> {
        self.step("write")?;
        // A short successful write must be completed before verification.
        self.temp.as_mut().unwrap().push(buf[0]);
        Ok(1)
    }

    fn progress(&mut self, _: u64) -> io::Result<()> {
        self.step("progress")
    }

    fn chmod(&mut self, _: &(), mode: u32) -> io::Result<()> {
        assert_eq!(mode, 0o755);
        self.step("chmod")
    }

    fn sync_file(&mut self, _: &()) -> io::Result<()> {
        self.step("file fsync")
    }

    fn rename(&mut self) -> io::Result<()> {
        self.step("rename")?;
        self.dest = self.temp.take().unwrap();
        Ok(())
    }

    fn sync_dir(&mut self) -> io::Result<()> {
        self.step("directory sync")
    }

    fn unlink_temp(&mut self) -> io::Result<()> {
        self.step("unlink")?;
        assert!(self.temp.take().is_some());
        Ok(())
    }

    fn open_staged(&mut self) -> io::Result<()> {
        self.step("open staged file")?;
        assert!(self.staged.is_some());
        Ok(())
    }

    fn read_staged(&mut self, _: &mut (), buf: &mut [u8]) -> io::Result<usize> {
        self.step("read staged file")?;
        let staged = self.staged.as_ref().unwrap();
        let n = buf.len().min(staged.len() - self.staged_read).min(2);
        buf[..n].copy_from_slice(&staged[self.staged_read..self.staged_read + n]);
        self.staged_read += n;
        Ok(n)
    }

    fn rename_staged(&mut self) -> io::Result<()> {
        self.step("rename staged file")?;
        self.dest = self.staged.take().unwrap();
        Ok(())
    }
}

#[test]
fn short_stdin_unlinks_temp_and_keeps_dest() {
    let mut fs = Fixture::new(b"ab", None);
    let error = publish(&mut fs, 3, DIGEST, 0o755).unwrap_err();
    assert_eq!(error.status, 1);
    assert!(error.reason.contains("short stdin"), "{error:?}");
    assert!(fs.calls.contains(&"open temporary file"));
    assert_eq!(fs.calls.last(), Some(&"unlink"));
    assert_eq!(fs.dest, OLD);
    assert!(fs.temp.is_none());
    assert!(!fs.calls.contains(&"rename"));
}

#[test]
fn every_precommit_failure_preserves_destination() {
    for step in [
        "open directory",
        "open temporary file",
        "stdin",
        "write",
        "progress",
        "chmod",
        "file fsync",
        "rename",
    ] {
        let mut fs = Fixture::new(NEW, Some(step));
        let error = publish(&mut fs, 3, DIGEST, 0o755).unwrap_err();
        assert_eq!(error.status, 1, "{step}: {error:?}");
        assert!(error.reason.contains(step), "{step}: {error:?}");
        assert_eq!(fs.dest, OLD, "{step}");
        assert!(fs.temp.is_none(), "{step}");
        assert!(!fs.calls.contains(&"directory sync"), "{step}");
        if !step.starts_with("open ") {
            assert_eq!(fs.calls.last(), Some(&"unlink"), "{step}");
        }
    }
    for (input, reason) in [(&b"abcd"[..], "long stdin"), (&b"xyz"[..], "SHA-256")] {
        let mut fs = Fixture::new(input, None);
        let error = publish(&mut fs, 3, DIGEST, 0o755).unwrap_err();
        assert_eq!(error.status, 1);
        assert!(error.reason.contains(reason), "{error:?}");
        assert_eq!(fs.dest, OLD);
        assert!(fs.temp.is_none());
        assert_eq!(fs.calls.last(), Some(&"unlink"));
    }
}

#[test]
fn rename_publishes_even_if_directory_sync_fails() {
    for fault in [None, Some("directory sync")] {
        let mut fs = Fixture::new(NEW, fault);
        let result = publish(&mut fs, 3, DIGEST, 0o755);
        if fault.is_some() {
            let error = result.unwrap_err();
            assert_eq!(error.status, 3);
            assert!(error.reason.contains("directory sync failed"), "{error:?}");
        } else {
            result.unwrap();
        }
        assert_eq!(fs.dest, NEW);
        assert!(fs.temp.is_none());
        assert!(fs
            .calls
            .ends_with(&["chmod", "file fsync", "rename", "directory sync"]));
    }
}

#[test]
fn empty_artifact_replaces_destination() {
    // SHA-256 of the empty byte string.
    let digest = [
        0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f, 0xb9,
        0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b, 0x78, 0x52,
        0xb8, 0x55,
    ];
    let mut fs = Fixture::new(b"", None);
    publish(&mut fs, 0, digest, 0o755).unwrap();
    assert!(fs.dest.is_empty());
    assert!(fs.temp.is_none());
}

#[test]
fn adopting_a_staged_upload_publishes_it_without_writing_it_again() {
    let mut fs = Fixture::staged(NEW, None);
    adopt(&mut fs, 3, DIGEST, 0o755).unwrap();
    assert_eq!(fs.dest, NEW);
    assert!(fs.staged.is_none());
    assert!(!fs.calls.contains(&"write"), "{:?}", fs.calls);
    assert!(!fs.calls.contains(&"open temporary file"), "{:?}", fs.calls);
    assert!(fs.calls.ends_with(&["chmod", "file fsync", "rename staged file", "directory sync"]));
}

#[test]
fn a_staged_upload_that_does_not_verify_is_not_published() {
    for (staged, reason) in [(&b"ab"[..], "short"), (&b"abcd"[..], "long"), (&b"xyz"[..], "SHA-256")] {
        let mut fs = Fixture::staged(staged, None);
        let error = adopt(&mut fs, 3, DIGEST, 0o755).unwrap_err();
        assert_eq!(error.status, 1);
        assert!(error.reason.contains(reason), "{error:?}");
        assert_eq!(fs.dest, OLD);
        // The staging directory's owner removes the file; bput leaves it.
        assert_eq!(fs.staged.as_deref(), Some(staged));
        assert!(!fs.calls.contains(&"rename staged file"));
    }
    for step in ["open directory", "open staged file", "read staged file", "chmod", "file fsync", "rename staged file"] {
        let mut fs = Fixture::staged(NEW, Some(step));
        let error = adopt(&mut fs, 3, DIGEST, 0o755).unwrap_err();
        assert_eq!(error.status, 1, "{step}: {error:?}");
        assert_eq!(fs.dest, OLD, "{step}");
    }
}
