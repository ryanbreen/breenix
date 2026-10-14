//! Linux sysinfo layout, populated from Breenix procfs without a new syscall.
use crate::pthread::{errno, set_errno};

#[repr(C)]
pub struct SysInfo {
    pub uptime: i64,
    pub loads: [u64; 3],
    pub totalram: u64,
    pub freeram: u64,
    pub sharedram: u64,
    pub bufferram: u64,
    pub totalswap: u64,
    pub freeswap: u64,
    pub procs: u16,
    pub totalhigh: u64,
    pub freehigh: u64,
    pub mem_unit: u32,
}
unsafe fn read_proc(path: &[u8], buf: &mut [u8]) -> Result<usize, ()> {
    let fd = crate::open(path.as_ptr(), 0x80000, 0);
    if fd < 0 {
        return Err(());
    }
    let mut used = 0;
    let result = loop {
        // Reserve one byte for an overflow probe: never parse a truncated file.
        let mut extra = [0u8; 1];
        let dest = if used == buf.len() {
            &mut extra[..]
        } else {
            &mut buf[used..]
        };
        let n = crate::read(fd, dest.as_mut_ptr(), dest.len());
        if n == 0 {
            break Ok(used);
        }
        if n < 0 {
            if errno() == 4 {
                continue;
            }
            break Err(());
        }
        if used == buf.len() {
            set_errno(75);
            break Err(());
        }
        used += n as usize;
    };
    let saved = errno();
    crate::close(fd);
    set_errno(saved);
    result
}
fn decimal(bytes: &[u8]) -> Option<u64> {
    if bytes.is_empty() {
        return None;
    }
    bytes.iter().try_fold(0u64, |n, b| {
        if !b.is_ascii_digit() {
            return None;
        }
        n.checked_mul(10)?.checked_add((b - b'0') as u64)
    })
}
fn memory(info: &mut SysInfo, text: &[u8]) -> Option<()> {
    let mut required = 0;
    for line in text.split(|b| *b == b'\n') {
        let mut fields = line
            .split(|b| b.is_ascii_whitespace())
            .filter(|s| !s.is_empty());
        let Some(key) = fields.next() else {
            continue;
        };
        let target = match key {
            b"MemTotal:" => {
                required |= 1;
                &mut info.totalram
            }
            b"MemFree:" => {
                required |= 2;
                &mut info.freeram
            }
            b"Shmem:" => &mut info.sharedram,
            b"Buffers:" => &mut info.bufferram,
            b"SwapTotal:" => &mut info.totalswap,
            b"SwapFree:" => &mut info.freeswap,
            _ => continue,
        };
        let n = decimal(fields.next()?)?;
        if fields.next() != Some(b"kB".as_slice()) {
            return None;
        }
        *target = n.checked_mul(1024)?;
    }
    if required == 3 {
        Some(())
    } else {
        None
    }
}
#[no_mangle]
pub unsafe extern "C" fn sysinfo(out: *mut SysInfo) -> i32 {
    if out.is_null() {
        set_errno(14);
        return -1;
    }
    let saved = errno();
    let mut info: SysInfo = core::mem::zeroed();
    info.mem_unit = 1;
    // Load averages and the process census are unavailable in these procfs files.
    let mut buf = [0u8; 4096];
    let Ok(n) = read_proc(b"/proc/meminfo\0", &mut buf) else {
        return -1;
    };
    if memory(&mut info, &buf[..n]).is_none() {
        set_errno(5);
        return -1;
    }
    let Ok(n) = read_proc(b"/proc/uptime\0", &mut buf) else {
        return -1;
    };
    let first = buf[..n]
        .split(|b| b.is_ascii_whitespace())
        .next()
        .unwrap_or(b"");
    let mut parts = first.split(|b| *b == b'.');
    let Some(seconds) = parts.next().and_then(decimal) else {
        set_errno(5);
        return -1;
    };
    let Some(fraction) = parts.next().and_then(decimal) else {
        set_errno(5);
        return -1;
    };
    let Some(uptime) = seconds
        .checked_add((fraction != 0) as u64)
        .and_then(|v| i64::try_from(v).ok())
    else {
        set_errno(75);
        return -1;
    };
    info.uptime = uptime;
    out.write(info);
    set_errno(saved);
    0
}
