//! POSIX sysconf, confstr, pathconf and fpathconf. Query IDs are defined by our
//! unistd.h, numbered as Linux numbers them where Linux has the name.
use crate::pthread::set_errno;

const EINVAL: i32 = 22;
const EFAULT: i32 = 14;

/// The value of an option this implementation supports: the POSIX.1-2024
/// version, as unistd.h defines it.
const VERSION: i64 = 202405;

/// A resource's soft limit, or -1 (with errno unchanged) when it has none.
fn soft_limit(resource: i32) -> i64 {
    let mut limit = crate::rlimit { rlim_cur: 0, rlim_max: 0 };
    if unsafe { crate::getrlimit(resource, &mut limit) } != 0 {
        return -1;
    }
    if limit.rlim_cur == u64::MAX {
        -1
    } else {
        limit.rlim_cur.min(i64::MAX as u64) as i64
    }
}

/// Physical memory in pages, all of it or the free part, from sysinfo.
fn memory_pages(free: bool) -> i64 {
    let mut info: crate::sysinfo::SysInfo = unsafe { core::mem::zeroed() };
    if unsafe { crate::sysinfo::sysinfo(&mut info) } != 0 {
        return -1;
    }
    let bytes = if free { info.freeram } else { info.totalram };
    (bytes.saturating_mul(info.mem_unit as u64) / 4096).min(i64::MAX as u64) as i64
}

/// sysconf's value for `name` (`crate::sysconf`): -1 with errno unchanged is a
/// valid name with no limit, or an option this implementation does not
/// support; -1 with EINVAL is an unknown name.
pub fn system_value(name: i32) -> i64 {
    const RLIMIT_NPROC: i32 = 6;
    const RLIMIT_NOFILE: i32 = 7;
    const RLIMIT_SIGPENDING: i32 = 11;
    match name {
        // The kernel reserves a quarter of its fixed 2 MiB stack cap for exec.
        0 => 512 * 1024,                     // ARG_MAX
        1 => soft_limit(RLIMIT_NPROC),        // CHILD_MAX
        2 => 100,                             // CLK_TCK: the kernel's USER_HZ
        3 => 65536,                           // NGROUPS_MAX
        4 | 5 => soft_limit(RLIMIT_NOFILE),   // OPEN_MAX; a stream needs only a descriptor
        6 => 6,                               // TZNAME_MAX
        7 | 8 => 1,                           // _POSIX_JOB_CONTROL, _POSIX_SAVED_IDS
        9 | 11 | 15 | 16 | 19 => VERSION,     // realtime signals, timers, fsync, mapped files, memory protection
        20 | 21 | 22 => VERSION,              // message passing, semaphores, shared memory objects
        26 => 0x7fff_ffff,                    // DELAYTIMER_MAX
        27 => -1,                             // MQ_OPEN_MAX: no fixed limit
        28 => 32768,                          // MQ_PRIO_MAX
        29 => VERSION,                        // _POSIX_VERSION
        30 => 4096,                           // PAGESIZE
        31 => 32,                             // RTSIG_MAX
        32 => 256,                            // SEM_NSEMS_MAX
        33 => 0x7fff_ffff,                    // SEM_VALUE_MAX
        34 => soft_limit(RLIMIT_SIGPENDING),  // SIGQUEUE_MAX
        35 => -1,                             // TIMER_MAX: no fixed limit
        36 | 38 => 99,                        // BC_BASE_MAX, BC_SCALE_MAX
        37 => 2048,                           // BC_DIM_MAX
        39 => 1000,                           // BC_STRING_MAX
        40 => 2,                              // COLL_WEIGHTS_MAX
        42 => 32,                             // EXPR_NEST_MAX
        43 => 2048,                           // LINE_MAX
        44 => 255,                            // RE_DUP_MAX
        46 => VERSION,                        // _POSIX2_VERSION
        60 => 1024,                           // IOV_MAX
        67 | 68 => VERSION,                   // threads, thread-safe functions
        69 | 70 => 1024,                      // GETGR_R_SIZE_MAX, GETPW_R_SIZE_MAX
        71 => 256,                            // LOGIN_NAME_MAX
        72 => 32,                             // TTY_NAME_MAX
        73 => 4,                              // PTHREAD_DESTRUCTOR_ITERATIONS
        74 => -1,                             // keys grow until allocation fails
        75 => 16384,                          // PTHREAD_STACK_MIN
        76 => -1,                             // PTHREAD_THREADS_MAX: no fixed limit
        77 | 78 => VERSION,                   // thread stack address and size attributes
        79 => -1,                             // priority-ordered wakeups are not provided
        80 | 81 => VERSION,                   // PTHREAD_PRIO_INHERIT and PTHREAD_PRIO_PROTECT
        83 | 84 => crate::pthread::processor_count(), // NPROCESSORS_CONF, NPROCESSORS_ONLN
        85 => memory_pages(false),            // PHYS_PAGES
        86 => memory_pages(true),             // AVPHYS_PAGES
        87 => 32,                             // ATEXIT_MAX
        89 => 800,                            // _XOPEN_VERSION
        109 => 20,                            // NZERO
        133 | 149 | 153 | 154 | 164 => VERSION, // barriers, monotonic clock, rwlocks, spin locks, timeouts
        155 | 157 => VERSION,                 // regular expressions, the shell
        173 => 40,                            // SYMLOOP_MAX
        180 => 255,                           // HOST_NAME_MAX
        // The programming environments: only LP64 with a 64-bit off_t.
        239 | V8_LP64_OFF64 => 1,
        237 | 238 | 240 | V8_ILP32_OFF32 | V8_ILP32_OFFBIG | V8_LPBIG_OFFBIG => -1,
        // Valid names of options this implementation does not provide.
        10 | 12 | 13 | 14 | 17 | 18 | 23 | 24 | 25 | 47..=52 | 82 | 90..=95 | 97 | 130 | 131
        | 132 | 137..=139 | 159..=161 | 165 | 235 | 236 | 241 | 247 | 248 => -1,
        _ => {
            set_errno(EINVAL);
            -1
        }
    }
}

/// sysconf names of the POSIX.1-2024 programming environments; Linux has none.
const V8_ILP32_OFF32: i32 = 250;
const V8_ILP32_OFFBIG: i32 = 251;
const V8_LP64_OFF64: i32 = 252;
const V8_LPBIG_OFFBIG: i32 = 253;

/// confstr names (unistd.h). The POSIX_V7 environments use Linux's numbers;
/// the POSIX_V8 ones, which Linux lacks, are numbered after them.
const CS_PATH: i32 = 0;
const CS_V7_WIDTH_RESTRICTED_ENVS: i32 = 5;
const CS_V7_ENVS: i32 = 1132; // ILP32_OFF32, ILP32_OFFBIG, LP64_OFF64, LPBIG_OFFBIG
const CS_V7_ENV: i32 = 1149;
const CS_V8_ENV: i32 = 1150;
const CS_V8_WIDTH_RESTRICTED_ENVS: i32 = 1151;
const CS_V8_THREADS_CFLAGS: i32 = 1152;
const CS_V8_THREADS_LDFLAGS: i32 = 1153;
const CS_V7_THREADS_CFLAGS: i32 = 1154;
const CS_V7_THREADS_LDFLAGS: i32 = 1155;
const CS_V8_ENVS: i32 = 1160; // as CS_V7_ENVS
const LP64_OFF64: i32 = 2;

/// A confstr name's value: Some(None) for a valid name with no value.
fn configuration_string(name: i32) -> Option<Option<&'static [u8]>> {
    Some(match name {
        CS_PATH => Some(b"/bin:/usr/bin\0"),
        CS_V7_WIDTH_RESTRICTED_ENVS => Some(b"POSIX_V7_LP64_OFF64\0"),
        CS_V8_WIDTH_RESTRICTED_ENVS => Some(b"POSIX_V8_LP64_OFF64\0"),
        // No environment variables select the environment.
        CS_V7_ENV | CS_V8_ENV => Some(b"\0"),
        // Threads and the supported environment need no extra flags.
        CS_V7_THREADS_CFLAGS | CS_V7_THREADS_LDFLAGS | CS_V8_THREADS_CFLAGS | CS_V8_THREADS_LDFLAGS => {
            Some(b"\0")
        }
        // CFLAGS, LDFLAGS and LIBS (and Linux's LINTFLAGS slot) of each
        // environment: defined for the one Breenix supports.
        _ if (CS_V7_ENVS..CS_V7_ENVS + 16).contains(&name) || (CS_V8_ENVS..CS_V8_ENVS + 16).contains(&name) => {
            let base = if name >= CS_V8_ENVS { CS_V8_ENVS } else { CS_V7_ENVS };
            let (environment, item) = ((name - base) / 4, (name - base) % 4);
            if item == 3 && base == CS_V8_ENVS {
                return None;
            }
            (environment == LP64_OFF64).then_some(b"\0".as_slice())
        }
        _ => return None,
    })
}

#[no_mangle]
pub unsafe extern "C" fn confstr(name: i32, buffer: *mut u8, len: usize) -> usize {
    let Some(value) = configuration_string(name) else {
        set_errno(EINVAL);
        return 0;
    };
    // A valid name with no value: 0, errno unchanged.
    let Some(value) = value else {
        return 0;
    };
    // Like Linux, a null buffer only asks for the length, whatever len says.
    if len != 0 && !buffer.is_null() {
        let copied = len.min(value.len()) - 1;
        core::ptr::copy_nonoverlapping(value.as_ptr(), buffer, copied);
        *buffer.add(copied) = 0;
    }
    value.len()
}
/// What a pathconf name reports for a file.
enum PathLimit {
    /// The same for every file.
    Fixed(i64),
    /// A property of the ext2 disk; indeterminate (-1) on a pseudo file system.
    Disk(i64),
    /// The file system's block size, less `less`: the smallest allocation
    /// (POSIX_ALLOC_SIZE_MIN) and, less one, the longest symbolic link ext2
    /// stores (SYMLINK_MAX).
    Block { less: i64 },
}

/// pathconf names (unistd.h): Linux's numbers, then _PC_TIMESTAMP_RESOLUTION.
fn limit(name: i32) -> Option<PathLimit> {
    Some(match name {
        0 => PathLimit::Disk(32000),          // LINK_MAX: ext2's enforced link limit
        // MAX_CANON, MAX_INPUT: the terminal line discipline's line buffer.
        1 => PathLimit::Fixed(4096),
        2 => PathLimit::Fixed(4095),
        3 => PathLimit::Fixed(255),           // NAME_MAX
        4 | 5 => PathLimit::Fixed(4096),      // PATH_MAX, PIPE_BUF
        6 | 7 => PathLimit::Fixed(1),         // CHOWN_RESTRICTED, NO_TRUNC
        8 => PathLimit::Fixed(0),             // VDISABLE
        // Synchronized, asynchronous and prioritized I/O: not provided.
        9 | 10 | 11 => PathLimit::Fixed(-1),
        12 => PathLimit::Fixed(-1),           // SOCK_MAXBUF: no limit
        13 => PathLimit::Disk(64),            // FILESIZEBITS
        14..=17 => PathLimit::Fixed(-1),      // POSIX_REC_*: no recommendation
        18 => PathLimit::Block { less: 0 },
        19 => PathLimit::Block { less: 1 },
        20 => PathLimit::Disk(1),             // POSIX2_SYMLINKS
        21 => PathLimit::Disk(1_000_000_000), // _POSIX_TIMESTAMP_RESOLUTION: ext2 keeps seconds
        _ => return None,
    })
}

/// Device numbers at or above this belong to the kernel's pseudo file systems
/// (procfs, devfs, pipes and the like), not to a mounted ext2 disk.
const PSEUDO_DEVICES: u64 = 0xffff_0000;

/// A path variable's value for the file `stat_file` and `statfs_file` find.
unsafe fn path_value(name: i32, stat_file: impl Fn(*mut u8) -> i32, statfs_file: impl Fn(*mut u8) -> i32) -> i64 {
    let Some(value) = limit(name) else {
        set_errno(EINVAL);
        return -1;
    };
    // stat follows symlinks and reports nonexistent or inaccessible paths.
    let mut stat = [0u64; 18];
    if stat_file(stat.as_mut_ptr().cast()) != 0 {
        return -1;
    }
    // struct stat starts with st_dev on both architectures.
    let disk = stat[0] < PSEUDO_DEVICES;
    match value {
        PathLimit::Fixed(value) => value,
        PathLimit::Disk(value) => if disk { value } else { -1 },
        PathLimit::Block { .. } if !disk => -1,
        PathLimit::Block { less } => {
            // struct statfs: f_type, then f_bsize.
            let mut fs = [0i64; 15];
            if statfs_file(fs.as_mut_ptr().cast()) != 0 {
                return -1;
            }
            fs[1] - less
        }
    }
}
#[no_mangle]
pub unsafe extern "C" fn pathconf(path: *const u8, name: i32) -> i64 {
    if limit(name).is_some() && path.is_null() {
        set_errno(EFAULT);
        return -1;
    }
    path_value(name, |buf| crate::stat(path, buf), |buf| crate::statfs::statfs(path, buf))
}
#[no_mangle]
pub unsafe extern "C" fn fpathconf(fd: i32, name: i32) -> i64 {
    path_value(name, |buf| crate::fstat(fd, buf), |buf| crate::statfs::fstatfs(fd, buf))
}
