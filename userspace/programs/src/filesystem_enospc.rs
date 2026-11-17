//! Reservation and freeing operations on a full scratch filesystem.
#[path = "filesystem_enospc_fill.rs"]
mod fill_observation;
use crate::filesystem_write::{on_scratch, tree};
use breenix_suite::suite::{check, CaseResult};
use libbreenix::{fs as sysfs, types::Fd};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::path::Path;

fn io<T>(result: std::io::Result<T>) -> Result<T, String> {
    result.map_err(|error| error.to_string())
}

fn stats(file: &File) -> Result<sysfs::Statfs, String> {
    sysfs::fstatfs(Fd::from_raw(file.as_raw_fd() as u64)).map_err(|e| e.to_string())
}

// A single write must either consume the whole chunk or report ENOSPC.
fn fill(file: &mut File, chunk: &[u8]) -> CaseResult {
    loop {
        match file.write(chunk) {
            Ok(n) if n == chunk.len() => (),
            Err(error) if error.raw_os_error() == Some(28) => return Ok(()),
            result => return Err(format!("fill write returned {result:?}").into()),
        }
    }
}

fn create_fill(root: &Path) -> Result<File, String> {
    io(OpenOptions::new().write(true).create_new(true).open(root.join("fill")))
}

pub fn fill_enospc_unchanged() -> CaseResult {
    on_scratch(|scratch| {
        let mut file = create_fill(&scratch.work)?;
        let chunk = vec![0x5a; 1024 * 1024];
        let mut expected = tree(&scratch.work)?;
        fill_observation::fill_and_observe(&mut file, &chunk, |accepted| {
            // Successful writes define the expected bytes. Read the actual full
            // tree at refusal, rather than copying its growing prefix per write.
            expected.get_mut(Path::new("fill")).ok_or("missing fill manifest entry")?
                .bytes.resize(accepted, 0x5a);
            if tree(&scratch.work)? != expected {
                return Err("failed write changed the tree".into());
            }
            Ok(())
        })?;
        drop(file);
        scratch.remount()?;
        check(tree(&scratch.work)? == expected, "failed write changed the remounted tree")
    })
}

pub fn delete_when_full() -> CaseResult {
    on_scratch(|scratch| {
        io(fs::write(scratch.work.join("unlink"), b"unlink"))?;
        io(fs::create_dir(scratch.work.join("empty")))?;
        io(fs::write(scratch.work.join("truncate"), b"truncate"))?;
        let truncate = io(OpenOptions::new().write(true).open(scratch.work.join("truncate")))?;
        let mut file = create_fill(&scratch.work)?;
        fill(&mut file, &[0x5a; 4096])?;
        io(fs::remove_file(scratch.work.join("unlink")))?;
        io(fs::remove_dir(scratch.work.join("empty")))?;
        io(truncate.set_len(0))?;
        io(file.sync_all())?;
        Ok(())
    })
}

pub fn bavail_excludes_reserve() -> CaseResult {
    on_scratch(|scratch| {
        let mut file = create_fill(&scratch.work)?;
        let before = stats(&file)?;
        check(before.f_blocks == 16384, "scratch volume is not 64 MiB")?;
        check(before.f_bfree >= 4096 && before.f_bavail <= before.f_bfree - 4096,
            &format!("reserve missing from statfs: {before:?}"))?;
        fill(&mut file, &[0x5a; 4096])?;
        let full = stats(&file)?;
        check(full.f_bavail == 0 && full.f_bfree >= 4096,
            &format!("full filesystem reserve differs: {full:?}"))
    })
}
