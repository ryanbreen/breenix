//! Host data ranges, rounded out to the breenfs allocation block.
use breenfs::format::BLOCK_SIZE;
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;

// Host lseek ABI values; these are not breenfs on-disk values.
#[cfg(target_os = "macos")]
const SEEK_DATA: i32 = 4;
#[cfg(target_os = "macos")]
const SEEK_HOLE: i32 = 3;
#[cfg(target_os = "linux")]
const SEEK_DATA: i32 = 3;
#[cfg(target_os = "linux")]
const SEEK_HOLE: i32 = 4;

extern "C" {
    fn lseek(fd: i32, offset: i64, whence: i32) -> i64;
}

fn seek(file: &File, offset: u64, whence: i32) -> io::Result<u64> {
    let offset = i64::try_from(offset).map_err(|_| io::Error::other("file too large"))?;
    // The descriptor is borrowed for this call; lseek does not retain it.
    let result = unsafe { lseek(file.as_raw_fd(), offset, whence) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result as u64)
    }
}

/// Return disjoint byte ranges to import. Unsupported seeking is an error,
/// rather than silently converting an unknown hole layout into allocated data.
pub fn ranges(file: &File, size: u64) -> io::Result<Vec<(u64, u64)>> {
    let block = BLOCK_SIZE as u64;
    let mut result: Vec<(u64, u64)> = Vec::new();
    let mut offset = 0;
    while offset < size {
        let data = match seek(file, offset, SEEK_DATA) {
            Ok(data) => data,
            Err(e) if e.raw_os_error() == Some(6) => break, // ENXIO: no more data.
            Err(e) => return Err(e),
        };
        if data >= size {
            break;
        }
        let hole = seek(file, data, SEEK_HOLE)?;
        if data < offset || hole <= data {
            return Err(io::Error::other("invalid SEEK_DATA/SEEK_HOLE range"));
        }
        let start = data / block * block;
        let end = hole.min(size).div_ceil(block) * block;
        if let Some(last) = result.last_mut().filter(|last| last.1 >= start) {
            last.1 = last.1.max(end);
        } else {
            result.push((start, end));
        }
        offset = hole;
    }
    Ok(result)
}
