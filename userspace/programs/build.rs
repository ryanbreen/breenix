use std::{env, fs, path::PathBuf};

fn main() {
    // libc is built separately by build.sh and linked as a native archive.
    // Cargo otherwise considers these binaries fresh when only libc changes.
    let arch = env::var("CARGO_CFG_TARGET_ARCH").expect("target architecture");
    let archive = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"))
        .join(format!(
            "../../libs/libbreenix-libc/target/{arch}-breenix/release/libc.a"
        ));
    println!("cargo:rerun-if-changed={}", archive.display());

    // `libc_has = "<name>"` for each thread, scheduling, signal and IPC function libc.a
    // defines (and the few others suite-threads and suite-ipc call), from the archive's
    // symbol index, so a suite can call a function the library has and fail a case
    // naming one it lacks, or make its system call, instead of failing to link.
    const PREFIXES: &[&str] = &["pthread_", "sched_", "sig", "mq_", "sem", "shm", "msg"];
    const OTHERS: &[&str] = &[
        "raise", "sysconf", "__errno_location", "exit", "getpid", "close", "pipe", "read", "write", "kill",
        "open", "unlink", "stat", "fstat", "lseek", "ftruncate", "mmap", "munmap", "umask", "poll", "select",
        "mkfifo", "ftok",
    ];
    println!("cargo:rustc-check-cfg=cfg(libc_has, values(any()))");
    if let Ok(bytes) = fs::read(&archive) {
        for name in archive_symbols(&bytes) {
            let wanted = PREFIXES.iter().any(|p| name.starts_with(p)) || OTHERS.contains(&name.as_str());
            if wanted && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                println!("cargo:rustc-cfg=libc_has=\"{name}\"");
            }
        }
    }
}

/// The names in a GNU ar archive's symbol index (the `/` or `/SYM64/` member): a
/// big-endian count, that many member offsets, then the names, each NUL-terminated.
fn archive_symbols(bytes: &[u8]) -> Vec<String> {
    const HEADER: usize = 60;
    if !bytes.starts_with(b"!<arch>\n") || bytes.len() < 8 + HEADER {
        return Vec::new();
    }
    let header = &bytes[8..8 + HEADER];
    let name = String::from_utf8_lossy(&header[..16]);
    let width = match name.trim_end() {
        "/" => 4,
        "/SYM64/" => 8,
        _ => return Vec::new(),
    };
    let Some(size) = String::from_utf8_lossy(&header[48..58]).trim().parse::<usize>().ok() else {
        return Vec::new();
    };
    let Some(body) = bytes.get(8 + HEADER..8 + HEADER + size) else { return Vec::new() };
    let Some(count) = body.get(..width).map(|b| b.iter().fold(0usize, |n, &x| n << 8 | x as usize)) else {
        return Vec::new();
    };
    let Some(names) = body.get(width + count * width..) else { return Vec::new() };
    names
        .split(|&b| b == 0)
        .filter(|n| !n.is_empty())
        .take(count)
        .map(|n| String::from_utf8_lossy(n).into_owned())
        .collect()
}
