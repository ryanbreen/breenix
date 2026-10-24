//! Process Filesystem (procfs)
//!
//! Provides a virtual filesystem mounted at /proc containing kernel and process
//! information. Unlike ext2, procfs doesn't use disk storage - all nodes are
//! virtual and generated on-demand.
//!
//! # Supported Entries
//!
//! ## Top-level files
//! - `/proc/uptime` - System uptime
//! - `/proc/version` - Kernel version
//! - `/proc/meminfo` - Memory statistics
//! - `/proc/cpuinfo` - CPU information
//!
//! ## Per-process entries (/proc/[pid]/)
//! - `/proc/[pid]/status` - Process name, state, parent, children, memory usage
//! - `/proc/[pid]/stat` - The same facts in Linux's one-line field layout
//! - `/proc/[pid]/cmdline` - The arguments the program was executed with
//! - `/proc/[pid]/exe` - Link to the executable
//!
//! ## Tracing entries (/proc/trace/)
//! - `/proc/trace/enable` - Tracing enable state (0/1)
//! - `/proc/trace/events` - List of available trace points
//! - `/proc/trace/buffer` - Trace buffer contents
//! - `/proc/trace/counters` - Trace counter values
//! - `/proc/trace/providers` - Registered trace providers
//!
//! # Architecture
//!
//! ```text
//! sys_open("/proc/uptime")
//!         |
//!         v
//!     procfs_open()
//!         |
//!         v
//!     lookup_entry("/proc/uptime")
//!         |
//!         v
//!     Generate content dynamically
//! ```

mod net;
mod pid_stat;
mod pid_path;

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

mod trace;
#[cfg(target_arch = "aarch64")]
mod xhci;

/// Procfs entry types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcEntryType {
    /// /proc/uptime - system uptime
    Uptime,
    /// /proc/version - kernel version
    Version,
    /// /proc/meminfo - memory information
    MemInfo,
    /// /proc/cpuinfo - CPU information
    CpuInfo,
    /// /proc/trace - trace directory (virtual)
    TraceDir,
    /// /proc/trace/enable - tracing enable state
    TraceEnable,
    /// /proc/trace/events - available trace events
    TraceEvents,
    /// /proc/trace/buffer - trace buffer contents
    TraceBuffer,
    /// /proc/trace/counters - trace counter values
    TraceCounters,
    /// /proc/trace/providers - registered providers
    TraceProviders,
    /// /proc/trace/teardown - per-PID teardown evidence directory
    TraceTeardownDir,
    /// /proc/trace/teardown/[pid] - per-PID teardown evidence
    TraceTeardownPid(u64),
    /// /proc/slabinfo - slab allocator statistics
    SlabInfo,
    /// /proc/stat - kernel activity counters
    Stat,
    /// /proc/cowinfo - copy-on-write statistics
    CowInfo,
    /// /proc/mounts - mounted filesystems
    Mounts,
    /// /proc/breenix - breenix info directory
    BreenixDir,
    /// /proc/breenix/testing - whether testing mode is active
    BreenixTesting,
    /// Durable clean/dirty state of the mounted disk filesystems.
    Filesystems,
    /// Suite-only stage fault handoff to the userspace renderer.
    StageControl,
    /// Actual producer markers reached in boot-path order.
    BootRibbon,
    BootSerial,
    StageComposites,
    ShowcasePace,
    /// /proc/breenix/ext2 - mounted ext2 allocation counters
    Ext2,
    /// /proc/breenix/frames - allocated physical frames by allocation site
    Frames,
    /// /proc/pids - list of all process IDs
    Pids,
    /// /proc/kmsg - kernel log messages
    Kmsg,
    /// /proc/kmsg-sequence - snapshot with absolute byte offsets
    KmsgSequence,
    /// /proc/xhci - xHCI directory
    XhciDir,
    /// /proc/xhci/trace - xHCI trace buffer
    XhciTrace,
    /// /proc/xhci/counters - xHCI trace counters
    XhciCounters,
    /// /proc/xhci/pointers - attached USB HID pointer interfaces
    XhciPointers,
    /// /proc/net - network directory
    NetDir,
    /// /proc/net/pnp - boot-time autoconfiguration, in Linux ipconfig's format
    NetPnp,
    /// /proc/net/tcp - TCP listeners and connections, the first columns of Linux's format
    NetTcp,
    /// /proc/console - screen console directory
    ConsoleDir,
    /// /proc/console/check - the screen read back against the console's shadow
    /// and font, where it was read, and the text rows it shows
    ConsoleCheck,
    /// /proc/fbcon - the x86-64 screen console's directory under the name the
    /// x86-metal boot path reads
    #[cfg(target_arch = "x86_64")]
    FbconDir,
    /// /proc/fbcon/check - the screen read back against the console's shadow
    /// and font, and the text rows it shows
    #[cfg(target_arch = "x86_64")]
    FbconCheck,
    /// /proc/[pid] - per-process directory (dynamic, not registered)
    PidDir(u64),
    /// /proc/[pid]/status - per-process status (dynamic, not registered)
    PidStatus(u64),
    /// /proc/[pid]/exe - executable pathname symlink
    PidExe(u64),
    /// /proc/[pid]/stat - per-process status in Linux's field layout
    PidStat(u64),
    /// /proc/[pid]/cmdline - the arguments of the program's exec
    PidCmdline(u64),
    /// /proc/self - symlink to the caller's process directory
    SelfLink,
    /// /proc/[pid]/task - a directory per thread of the process
    PidTasks(u64),
    /// /proc/[pid]/task/[tid] - one thread's directory
    PidTask(u64, u64),
    /// /proc/[pid]/task/[tid]/kernel_output - the thread's kernel output bytes
    /// (docs/gui/breengel.md section 22)
    PidTaskKernelOutput(u64, u64),
}

impl ProcEntryType {
    /// Get the entry name (without path prefix)
    ///
    /// Note: For dynamic entries (PidDir, PidStatus) this returns a static
    /// placeholder. Use `name_string()` for a dynamic name.
    pub fn name(&self) -> &'static str {
        match self {
            ProcEntryType::Uptime => "uptime",
            ProcEntryType::Version => "version",
            ProcEntryType::MemInfo => "meminfo",
            ProcEntryType::CpuInfo => "cpuinfo",
            ProcEntryType::TraceDir => "trace",
            ProcEntryType::TraceEnable => "enable",
            ProcEntryType::TraceEvents => "events",
            ProcEntryType::TraceBuffer => "buffer",
            ProcEntryType::TraceCounters => "counters",
            ProcEntryType::TraceProviders => "providers",
            ProcEntryType::TraceTeardownDir => "teardown",
            ProcEntryType::TraceTeardownPid(_) => "pid",
            ProcEntryType::SlabInfo => "slabinfo",
            ProcEntryType::Stat => "stat",
            ProcEntryType::CowInfo => "cowinfo",
            ProcEntryType::Mounts => "mounts",
            ProcEntryType::BreenixDir => "breenix",
            ProcEntryType::BreenixTesting => "testing",
            ProcEntryType::StageControl => "stage-control",
            ProcEntryType::ShowcasePace => "pace",
            ProcEntryType::BootRibbon => "boot-ribbon",
            ProcEntryType::BootSerial => "boot-serial",
            ProcEntryType::StageComposites => "stage-composites",
            ProcEntryType::Ext2 => "ext2",
            ProcEntryType::Filesystems => "filesystems",
            ProcEntryType::Frames => "frames",
            ProcEntryType::Pids => "pids",
            ProcEntryType::Kmsg => "kmsg",
            ProcEntryType::KmsgSequence => "kmsg-sequence",
            ProcEntryType::XhciDir => "xhci",
            ProcEntryType::XhciTrace => "trace",
            ProcEntryType::XhciCounters => "counters",
            ProcEntryType::XhciPointers => "pointers",
            ProcEntryType::NetDir => "net",
            ProcEntryType::NetPnp => "pnp",
            ProcEntryType::NetTcp => "tcp",
            ProcEntryType::ConsoleDir => "console",
            ProcEntryType::ConsoleCheck => "check",
            #[cfg(target_arch = "x86_64")]
            ProcEntryType::FbconDir => "fbcon",
            #[cfg(target_arch = "x86_64")]
            ProcEntryType::FbconCheck => "check",
            ProcEntryType::PidDir(_) => "pid",
            ProcEntryType::PidStatus(_) => "status",
            ProcEntryType::PidExe(_) => "exe",
            ProcEntryType::PidStat(_) => "stat",
            ProcEntryType::PidCmdline(_) => "cmdline",
            ProcEntryType::SelfLink => "self",
            ProcEntryType::PidTasks(_) => "task",
            ProcEntryType::PidTask(_, _) => "tid",
            ProcEntryType::PidTaskKernelOutput(_, _) => "kernel_output",
        }
    }

    /// Get the full path
    ///
    /// Note: For dynamic entries (PidDir, PidStatus) this returns a
    /// placeholder since the real path depends on the PID.
    pub fn path(&self) -> &'static str {
        match self {
            ProcEntryType::Uptime => "/proc/uptime",
            ProcEntryType::Version => "/proc/version",
            ProcEntryType::MemInfo => "/proc/meminfo",
            ProcEntryType::CpuInfo => "/proc/cpuinfo",
            ProcEntryType::TraceDir => "/proc/trace",
            ProcEntryType::TraceEnable => "/proc/trace/enable",
            ProcEntryType::TraceEvents => "/proc/trace/events",
            ProcEntryType::TraceBuffer => "/proc/trace/buffer",
            ProcEntryType::TraceCounters => "/proc/trace/counters",
            ProcEntryType::TraceProviders => "/proc/trace/providers",
            ProcEntryType::TraceTeardownDir => "/proc/trace/teardown",
            ProcEntryType::TraceTeardownPid(_) => "/proc/trace/teardown/<pid>",
            ProcEntryType::SlabInfo => "/proc/slabinfo",
            ProcEntryType::Stat => "/proc/stat",
            ProcEntryType::CowInfo => "/proc/cowinfo",
            ProcEntryType::Mounts => "/proc/mounts",
            ProcEntryType::BreenixDir => "/proc/breenix",
            ProcEntryType::BreenixTesting => "/proc/breenix/testing",
            ProcEntryType::StageControl => "/proc/breenix/stage-control",
            ProcEntryType::ShowcasePace => "/proc/breenix/pace",
            ProcEntryType::BootRibbon => "/proc/breenix/boot-ribbon",
            ProcEntryType::BootSerial => "/proc/breenix/boot-serial",
            ProcEntryType::StageComposites => "/proc/breenix/stage-composites",
            ProcEntryType::Ext2 => "/proc/breenix/ext2",
            ProcEntryType::Filesystems => "/proc/breenix/filesystems",
            ProcEntryType::Frames => "/proc/breenix/frames",
            ProcEntryType::Pids => "/proc/pids",
            ProcEntryType::Kmsg => "/proc/kmsg",
            ProcEntryType::KmsgSequence => "/proc/kmsg-sequence",
            ProcEntryType::XhciDir => "/proc/xhci",
            ProcEntryType::XhciTrace => "/proc/xhci/trace",
            ProcEntryType::XhciCounters => "/proc/xhci/counters",
            ProcEntryType::XhciPointers => "/proc/xhci/pointers",
            ProcEntryType::NetDir => "/proc/net",
            ProcEntryType::NetPnp => "/proc/net/pnp",
            ProcEntryType::NetTcp => "/proc/net/tcp",
            ProcEntryType::ConsoleDir => "/proc/console",
            ProcEntryType::ConsoleCheck => "/proc/console/check",
            #[cfg(target_arch = "x86_64")]
            ProcEntryType::FbconDir => "/proc/fbcon",
            #[cfg(target_arch = "x86_64")]
            ProcEntryType::FbconCheck => "/proc/fbcon/check",
            // Dynamic entries don't have static paths
            ProcEntryType::PidDir(_) => "/proc/<pid>",
            ProcEntryType::PidStatus(_) => "/proc/<pid>/status",
            ProcEntryType::PidExe(_) => "/proc/<pid>/exe",
            ProcEntryType::PidStat(_) => "/proc/<pid>/stat",
            ProcEntryType::PidCmdline(_) => "/proc/<pid>/cmdline",
            ProcEntryType::SelfLink => "/proc/self",
            ProcEntryType::PidTasks(_) => "/proc/<pid>/task",
            ProcEntryType::PidTask(_, _) => "/proc/<pid>/task/<tid>",
            ProcEntryType::PidTaskKernelOutput(_, _) => "/proc/<pid>/task/<tid>/kernel_output",
        }
    }

    /// Get the inode number for this entry
    ///
    /// Dynamic PID entries use computed inodes above every static one, eight
    /// per PID, so no two entries share a number whatever the PID.
    pub fn inode(&self) -> u64 {
        match self {
            ProcEntryType::Uptime => 1,
            ProcEntryType::Version => 2,
            ProcEntryType::MemInfo => 3,
            ProcEntryType::CpuInfo => 4,
            ProcEntryType::TraceDir => 100,
            ProcEntryType::TraceEnable => 101,
            ProcEntryType::TraceEvents => 102,
            ProcEntryType::TraceBuffer => 103,
            ProcEntryType::TraceCounters => 104,
            ProcEntryType::TraceProviders => 105,
            ProcEntryType::TraceTeardownDir => 106,
            ProcEntryType::TraceTeardownPid(pid) => pid_inode(*pid, 3),
            ProcEntryType::SlabInfo => 5,
            ProcEntryType::Stat => 6,
            ProcEntryType::CowInfo => 7,
            ProcEntryType::Mounts => 8,
            ProcEntryType::BreenixDir => 200,
            ProcEntryType::BreenixTesting => 201,
            ProcEntryType::StageControl => 208,
            ProcEntryType::ShowcasePace => 207,
            ProcEntryType::BootRibbon => 204,
            ProcEntryType::BootSerial => 205,
            ProcEntryType::StageComposites => 206,
            ProcEntryType::Ext2 => 202,
            ProcEntryType::Filesystems => 209,
            ProcEntryType::Frames => 203,
            ProcEntryType::Pids => 9,
            ProcEntryType::Kmsg => 10,
            ProcEntryType::KmsgSequence => 31,
            ProcEntryType::XhciDir => 300,
            ProcEntryType::XhciTrace => 301,
            ProcEntryType::XhciCounters => 302,
            ProcEntryType::XhciPointers => 303,
            ProcEntryType::NetDir => 400,
            ProcEntryType::NetPnp => 401,
            ProcEntryType::NetTcp => 402,
            #[cfg(target_arch = "x86_64")]
            ProcEntryType::FbconDir => 500,
            #[cfg(target_arch = "x86_64")]
            ProcEntryType::FbconCheck => 501,
            ProcEntryType::ConsoleDir => 502,
            ProcEntryType::ConsoleCheck => 503,
            ProcEntryType::PidDir(pid) => pid_inode(*pid, 0),
            ProcEntryType::PidStatus(pid) => pid_inode(*pid, 1),
            ProcEntryType::PidExe(pid) => pid_inode(*pid, 2),
            ProcEntryType::PidStat(pid) => pid_inode(*pid, 4),
            ProcEntryType::PidCmdline(pid) => pid_inode(*pid, 5),
            ProcEntryType::SelfLink => 11,
            ProcEntryType::PidTasks(pid) => pid_inode(*pid, 6),
            ProcEntryType::PidTask(_, tid) => task_inode(*tid, 0),
            ProcEntryType::PidTaskKernelOutput(_, tid) => task_inode(*tid, 1),
        }
    }

    /// Check if this is a symbolic link
    pub fn is_link(&self) -> bool {
        matches!(self, ProcEntryType::SelfLink | ProcEntryType::PidExe(_))
    }

    /// Check if this is a directory
    pub fn is_directory(&self) -> bool {
        if matches!(self, ProcEntryType::ConsoleDir) {
            return true;
        }
        #[cfg(target_arch = "x86_64")]
        if matches!(self, ProcEntryType::FbconDir) {
            return true;
        }
        matches!(
            self,
            ProcEntryType::TraceDir
                | ProcEntryType::TraceTeardownDir
                | ProcEntryType::BreenixDir
                | ProcEntryType::XhciDir
                | ProcEntryType::NetDir
                | ProcEntryType::PidDir(_)
                | ProcEntryType::PidTasks(_)
                | ProcEntryType::PidTask(_, _)
        )
    }
}

/// The inode of a per-process entry: `slot` 0 is the directory, 1 status, 2
/// exe, 3 the teardown trace, 4 stat and 5 cmdline.
pub fn pid_inode(pid: u64, slot: u64) -> u64 {
    (1u64 << 32).wrapping_add(pid.wrapping_mul(8)).wrapping_add(slot)
}

/// The inode of a per-thread entry: `slot` 0 is `/proc/<pid>/task/<tid>`, 1 its
/// `kernel_output`. Thread ids get their own range above the per-process inodes.
pub fn task_inode(tid: u64, slot: u64) -> u64 {
    (1u64 << 62).wrapping_add(tid.wrapping_mul(2)).wrapping_add(slot)
}

/// The names in a process directory, in listing order.
pub const PID_DIR_ENTRIES: [&str; 5] = ["status", "stat", "cmdline", "exe", "task"];

/// A procfs entry node
#[derive(Debug, Clone)]
pub struct ProcEntry {
    /// Entry type
    pub entry_type: ProcEntryType,
}

impl ProcEntry {
    /// Create a new proc entry
    pub fn new(entry_type: ProcEntryType) -> Self {
        Self { entry_type }
    }
}

/// Global procfs state
struct ProcfsState {
    /// Registered entries
    entries: Vec<ProcEntry>,
    /// Whether procfs is initialized
    initialized: bool,
}

impl ProcfsState {
    const fn new() -> Self {
        Self {
            entries: Vec::new(),
            initialized: false,
        }
    }
}

static PROCFS: Mutex<ProcfsState> = Mutex::new(ProcfsState::new());

/// Initialize procfs with standard entries
pub fn init() {
    let mut procfs = PROCFS.lock();
    if procfs.initialized {
        log::warn!("procfs already initialized");
        return;
    }

    // Register standard /proc entries
    procfs.entries.push(ProcEntry::new(ProcEntryType::Uptime));
    procfs.entries.push(ProcEntry::new(ProcEntryType::Version));
    procfs.entries.push(ProcEntry::new(ProcEntryType::MemInfo));
    procfs.entries.push(ProcEntry::new(ProcEntryType::CpuInfo));
    procfs.entries.push(ProcEntry::new(ProcEntryType::SlabInfo));
    procfs.entries.push(ProcEntry::new(ProcEntryType::Stat));
    procfs.entries.push(ProcEntry::new(ProcEntryType::CowInfo));
    procfs.entries.push(ProcEntry::new(ProcEntryType::Mounts));

    // Register /proc/breenix directory and entries
    procfs
        .entries
        .push(ProcEntry::new(ProcEntryType::BreenixDir));
    procfs
        .entries
        .push(ProcEntry::new(ProcEntryType::BreenixTesting));

    procfs.entries.push(ProcEntry::new(ProcEntryType::StageControl));
    procfs.entries.push(ProcEntry::new(ProcEntryType::ShowcasePace));
    procfs.entries.push(ProcEntry::new(ProcEntryType::BootRibbon));
    procfs.entries.push(ProcEntry::new(ProcEntryType::BootSerial));
    procfs.entries.push(ProcEntry::new(ProcEntryType::StageComposites));
    procfs.entries.push(ProcEntry::new(ProcEntryType::Ext2));
    procfs.entries.push(ProcEntry::new(ProcEntryType::Frames));
    procfs.entries.push(ProcEntry::new(ProcEntryType::Filesystems));
    procfs.entries.push(ProcEntry::new(ProcEntryType::Pids));
    procfs.entries.push(ProcEntry::new(ProcEntryType::Kmsg));
    procfs.entries.push(ProcEntry::new(ProcEntryType::KmsgSequence));
    procfs.entries.push(ProcEntry::new(ProcEntryType::SelfLink));

    // Register /proc/xhci directory and entries
    procfs.entries.push(ProcEntry::new(ProcEntryType::XhciDir));
    procfs
        .entries
        .push(ProcEntry::new(ProcEntryType::XhciTrace));
    procfs
        .entries
        .push(ProcEntry::new(ProcEntryType::XhciCounters));
    procfs
        .entries
        .push(ProcEntry::new(ProcEntryType::XhciPointers));

    // Register /proc/net directory and entries
    procfs.entries.push(ProcEntry::new(ProcEntryType::NetDir));
    procfs.entries.push(ProcEntry::new(ProcEntryType::NetPnp));
    procfs.entries.push(ProcEntry::new(ProcEntryType::NetTcp));

    // Register the screen console's directories and entries
    procfs.entries.push(ProcEntry::new(ProcEntryType::ConsoleDir));
    procfs.entries.push(ProcEntry::new(ProcEntryType::ConsoleCheck));
    #[cfg(target_arch = "x86_64")]
    {
        procfs.entries.push(ProcEntry::new(ProcEntryType::FbconDir));
        procfs.entries.push(ProcEntry::new(ProcEntryType::FbconCheck));
    }

    // Register /proc/trace directory and entries
    procfs.entries.push(ProcEntry::new(ProcEntryType::TraceDir));
    procfs
        .entries
        .push(ProcEntry::new(ProcEntryType::TraceEnable));
    procfs
        .entries
        .push(ProcEntry::new(ProcEntryType::TraceEvents));
    procfs
        .entries
        .push(ProcEntry::new(ProcEntryType::TraceBuffer));
    procfs
        .entries
        .push(ProcEntry::new(ProcEntryType::TraceCounters));
    procfs
        .entries
        .push(ProcEntry::new(ProcEntryType::TraceProviders));
    procfs
        .entries
        .push(ProcEntry::new(ProcEntryType::TraceTeardownDir));

    procfs.initialized = true;
    log::info!("procfs: initialized with {} entries", procfs.entries.len());

    // Register mount point
    crate::fs::vfs::mount::mount("/proc", "procfs");
}

/// Look up an entry by path (without /proc prefix)
pub fn lookup(name: &str) -> Option<ProcEntry> {
    // Try static entries first (under PROCFS lock)
    {
        let procfs = PROCFS.lock();

        // Handle paths like "trace/enable" or just "uptime"
        let normalized = name.trim_start_matches('/');

        for entry in &procfs.entries {
            let entry_path = entry.entry_type.path();
            // Check if the full path matches or the relative path matches
            let relative = entry_path.trim_start_matches("/proc/");
            if relative == normalized || entry.entry_type.name() == normalized {
                return Some(entry.clone());
            }
        }
    }
    // PROCFS lock is released here before we try dynamic PID lookup

    // Try dynamic per-PID paths (e.g., "123/status", "123")
    let normalized = name.trim_start_matches('/');
    if let Some(entry) = lookup_trace_teardown_path(normalized) {
        return Some(entry);
    }
    let full_path = alloc::format!("/proc/{}", normalized);
    lookup_pid_path(&full_path)
}

/// Look up an entry by full path
pub fn lookup_by_path(path: &str) -> Option<ProcEntry> {
    // Try static entries first (under PROCFS lock)
    {
        let procfs = PROCFS.lock();
        for entry in &procfs.entries {
            if entry.entry_type.path() == path {
                return Some(entry.clone());
            }
        }
    }
    // PROCFS lock is released here before we try dynamic PID lookup

    // Try dynamic per-PID paths (e.g., /proc/123/status, /proc/123)
    lookup_trace_teardown_path(path.trim_start_matches("/proc/")).or_else(|| lookup_pid_path(path))
}

/// Look up an entry by inode number
pub fn lookup_by_inode(inode: u64) -> Option<ProcEntry> {
    let procfs = PROCFS.lock();

    for entry in &procfs.entries {
        if entry.entry_type.inode() == inode {
            return Some(entry.clone());
        }
    }
    None
}

/// List all entries (for /proc directory listing)
///
/// Returns static entries plus a directory per process: one per thread
/// group, named by the PID getpid reports, as Linux lists /proc. ENOMEM when
/// the listing cannot be allocated.
pub fn list_entries() -> Result<Vec<String>, i32> {
    use core::fmt::Write;
    const ENOMEM: i32 = 12;

    // Collect static entries under PROCFS lock
    let mut entries: Vec<String> = Vec::new();
    {
        let procfs = PROCFS.lock();
        for e in procfs.entries.iter().filter(|e| {
            !matches!(
                e.entry_type,
                ProcEntryType::TraceEnable
                    | ProcEntryType::TraceEvents
                    | ProcEntryType::TraceBuffer
                    | ProcEntryType::TraceCounters
                    | ProcEntryType::TraceProviders
                    | ProcEntryType::TraceTeardownDir
                    | ProcEntryType::BreenixTesting
                    | ProcEntryType::Filesystems
                    | ProcEntryType::Frames
                    | ProcEntryType::XhciTrace
                    | ProcEntryType::XhciCounters
                    | ProcEntryType::XhciPointers
                    | ProcEntryType::NetPnp
                    | ProcEntryType::NetTcp
            ) && !is_console_file(&e.entry_type)
        }) {
            let name = e.entry_type.name();
            let mut text = String::new();
            text.try_reserve_exact(name.len()).map_err(|_| ENOMEM)?;
            text.push_str(name);
            entries.try_reserve(1).map_err(|_| ENOMEM)?;
            entries.push(text);
        }
    }
    // PROCFS lock is released here before acquiring process manager lock

    // Add a directory per thread group from the process manager. The
    // manager's map is ordered by PID, so the listing is too.
    let manager_guard = crate::process::manager();
    if let Some(ref manager) = *manager_guard {
        for (pid, process) in manager.iter_processes() {
            if process.thread_group_id.is_some_and(|tgid| tgid != pid.as_u64()) {
                continue;
            }
            let mut text = String::new();
            text.try_reserve_exact(20).map_err(|_| ENOMEM)?;
            write!(text, "{}", pid.as_u64()).map_err(|_| ENOMEM)?;
            entries.try_reserve(1).map_err(|_| ENOMEM)?;
            entries.push(text);
        }
    }

    Ok(entries)
}

/// The entry `name` in the procfs directory `dir`, found without allocating.
/// Per-process entries are not checked against the process table: a listing
/// names only what it found there.
pub fn directory_child(dir: &str, name: &str) -> Option<ProcEntryType> {
    if dir == "/proc" && !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()) {
        return name.parse().ok().map(ProcEntryType::PidDir);
    }
    if let Some(pid) = dir.strip_prefix("/proc/").and_then(|tail| tail.parse::<u64>().ok()) {
        return match name {
            "status" => Some(ProcEntryType::PidStatus(pid)),
            "stat" => Some(ProcEntryType::PidStat(pid)),
            "cmdline" => Some(ProcEntryType::PidCmdline(pid)),
            "exe" => Some(ProcEntryType::PidExe(pid)),
            "task" => Some(ProcEntryType::PidTasks(pid)),
            _ => None,
        };
    }
    if let Some((pid, rest)) = dir
        .strip_prefix("/proc/")
        .and_then(|tail| tail.split_once("/task"))
        .and_then(|(pid, rest)| Some((pid.parse::<u64>().ok()?, rest)))
    {
        return match rest {
            "" => name.parse().ok().map(|tid| ProcEntryType::PidTask(pid, tid)),
            _ => {
                let tid = rest.strip_prefix('/')?.parse().ok()?;
                (name == "kernel_output").then_some(ProcEntryType::PidTaskKernelOutput(pid, tid))
            }
        };
    }
    let procfs = PROCFS.lock();
    procfs
        .entries
        .iter()
        .find(|e| {
            e.entry_type
                .path()
                .strip_prefix(dir)
                .and_then(|rest| rest.strip_prefix('/'))
                == Some(name)
        })
        .map(|e| e.entry_type)
}

/// The listing of `/proc/<pid>/task` (its threads) or `/proc/<pid>/task/<tid>`; other
/// directories are not listed here.
pub fn list_task_entries(dir: &str) -> Option<Result<Vec<String>, i32>> {
    let (pid, rest) = dir.strip_prefix("/proc/")?.split_once("/task")?;
    let pid: u64 = pid.parse().ok()?;
    if rest.is_empty() {
        return Some(thread_ids(pid).map(|ids| ids.iter().map(|tid| alloc::format!("{tid}")).collect()));
    }
    rest.strip_prefix('/')?.parse::<u64>().ok()?;
    Some(Ok(alloc::vec![String::from("kernel_output")]))
}

/// List entries in the /proc/trace directory
pub fn list_trace_entries() -> Vec<String> {
    let procfs = PROCFS.lock();
    procfs
        .entries
        .iter()
        .filter(|e| {
            matches!(
                e.entry_type,
                ProcEntryType::TraceEnable
                    | ProcEntryType::TraceEvents
                    | ProcEntryType::TraceBuffer
                    | ProcEntryType::TraceCounters
                    | ProcEntryType::TraceProviders
                    | ProcEntryType::TraceTeardownDir
            )
        })
        .map(|e| String::from(e.entry_type.name()))
        .collect()
}

/// List entries in the /proc/xhci directory
pub fn list_xhci_entries() -> Vec<String> {
    let procfs = PROCFS.lock();
    procfs
        .entries
        .iter()
        .filter(|e| {
            matches!(
                e.entry_type,
                ProcEntryType::XhciTrace
                    | ProcEntryType::XhciCounters
                    | ProcEntryType::XhciPointers
            )
        })
        .map(|e| String::from(e.entry_type.name()))
        .collect()
}

/// Whether `entry` lives inside a screen console directory rather than in
/// /proc itself.
fn is_console_file(entry: &ProcEntryType) -> bool {
    #[cfg(target_arch = "x86_64")]
    if matches!(entry, ProcEntryType::FbconCheck) {
        return true;
    }
    matches!(entry, ProcEntryType::ConsoleCheck)
}

/// List entries in the /proc/net directory
pub fn list_net_entries() -> Result<Vec<String>, i32> {
    net::list_entries()
}

/// List entries in the /proc/breenix directory
pub fn list_breenix_entries() -> Vec<String> {
    let procfs = PROCFS.lock();
    procfs
        .entries
        .iter()
        .filter(|e| matches!(e.entry_type, ProcEntryType::BreenixTesting|ProcEntryType::ShowcasePace|ProcEntryType::StageControl|ProcEntryType::BootRibbon|ProcEntryType::BootSerial|ProcEntryType::StageComposites))
        .map(|e| String::from(e.entry_type.name()))
        .collect()
}

/// Check if procfs is initialized
pub fn is_initialized() -> bool {
    PROCFS.lock().initialized
}

/// Read a procfs entry and return its content as a string
///
/// # Arguments
/// * `entry_type` - The type of entry to read
///
/// # Returns
/// The content as a String, or an error code
pub fn read_entry(entry_type: ProcEntryType) -> Result<Vec<u8>, i32> {
    match entry_type {
        ProcEntryType::ShowcasePace => {
            #[cfg(target_arch="aarch64")]
            if !crate::platform_config::is_qemu() { return text(String::from("\n")); }
            let pace=crate::drivers::fw_cfg::read_string("opt/breenix/pace").unwrap_or_default();
            text(alloc::format!("{}\n",pace))
        }
        #[cfg(target_arch="aarch64")]
        ProcEntryType::StageControl => {
            if !crate::test_control::enabled() {return Err(-38);}
            let caller=crate::window::Caller::current().map_err(|e|-e)?;
            if !caller.is_root(){return Err(-1);}
            let mut bits=0u32;
            for point in [breengel_abi::tc_point::PANEL_CURSOR_NO_RESTORE,12,13,14] {if crate::test_control::take(point,caller.tgid).is_some(){bits|=1<<point;}}
            text(alloc::format!("{bits}\n"))
        }
        #[cfg(not(target_arch="aarch64"))]
        ProcEntryType::StageControl => Err(-38),
        ProcEntryType::BootRibbon => {
            #[cfg(target_arch="aarch64")]
            {text(crate::graphics::boot_ribbon::records())}
            #[cfg(not(target_arch="aarch64"))]
            {Err(-38)}
        }
        ProcEntryType::BootSerial => {
            #[cfg(target_arch="aarch64")]
            { crate::graphics::boot_ribbon::serial().map(|s| s.into_bytes()) }
            #[cfg(not(target_arch="aarch64"))]
            {Err(-38)}
        }
        // The display owners' completed GPU presents (the run stage's screen changes).
        ProcEntryType::StageComposites => {
            #[cfg(target_arch="aarch64")]
            {text(crate::gpu::present_records())}
            #[cfg(not(target_arch="aarch64"))]
            {Err(-38)}
        }
        ProcEntryType::Uptime => text(generate_uptime()),
        ProcEntryType::Version => text(generate_version()),
        ProcEntryType::MemInfo => text(generate_meminfo()),
        ProcEntryType::CpuInfo => text(generate_cpuinfo()),
        // The tracing framework sees other users' processes: root only.
        ProcEntryType::TraceDir => {
            require(Operation::Trace)?;
            // Directory listing
            let entries = list_trace_entries();
            text(entries.join("\n") + "\n")
        }
        ProcEntryType::TraceEnable => require(Operation::Trace).and_then(|()| text(trace::generate_enable())),
        ProcEntryType::TraceEvents => require(Operation::Trace).and_then(|()| text(trace::generate_events())),
        ProcEntryType::TraceBuffer => require(Operation::Trace).and_then(|()| text(trace::generate_buffer())),
        ProcEntryType::TraceCounters => require(Operation::Trace).and_then(|()| text(trace::generate_counters())),
        ProcEntryType::TraceProviders => require(Operation::Trace).and_then(|()| text(trace::generate_providers())),
        ProcEntryType::TraceTeardownDir => require(Operation::Trace).and_then(|()| text(String::from("<pid>\n"))),
        ProcEntryType::TraceTeardownPid(pid) => {
            require(Operation::Trace).and_then(|()| text(trace::generate_teardown_pid(pid)))
        }
        ProcEntryType::SlabInfo => text(generate_slabinfo()),
        ProcEntryType::Stat => text(generate_stat()),
        ProcEntryType::CowInfo => text(generate_cowinfo()),
        ProcEntryType::Pids => text(generate_pids()),
        ProcEntryType::KmsgSequence => {
            require(Operation::KernelLog)?;
            text(crate::log_buffer::sequenced_snapshot())
        },
        ProcEntryType::Kmsg => {
            // The serial ring can contain privileged TTY output. Authorize
            // before generating the open-time snapshot, as for console/check.
            require(Operation::KernelLog)?;
            text(generate_kmsg())
        },
        ProcEntryType::Mounts => text(generate_mounts()),
        ProcEntryType::XhciDir => {
            let entries = list_xhci_entries();
            text(entries.join("\n") + "\n")
        }
        #[cfg(target_arch = "aarch64")]
        ProcEntryType::XhciTrace => text(xhci::generate_xhci_trace()),
        #[cfg(target_arch = "aarch64")]
        ProcEntryType::XhciCounters => text(xhci::generate_xhci_counters()),
        #[cfg(not(target_arch = "aarch64"))]
        ProcEntryType::XhciTrace => text(String::from("")),
        #[cfg(not(target_arch = "aarch64"))]
        ProcEntryType::XhciCounters => text(String::from("")),
        #[cfg(target_arch = "aarch64")]
        ProcEntryType::XhciPointers => text(crate::drivers::xhci::describe_pointers()),
        #[cfg(not(target_arch = "aarch64"))]
        ProcEntryType::XhciPointers => text(String::from("")),
        ProcEntryType::Ext2 => generate_ext2().map(String::into_bytes),
        ProcEntryType::Filesystems => crate::fs::ext2::filesystem_states().map(String::into_bytes).map_err(|_| -5),
        ProcEntryType::Frames => generate_frames().map(String::into_bytes),
        ProcEntryType::NetDir => {
            let mut listing = Vec::new();
            listing.try_reserve_exact(8).map_err(|_| -12)?;
            listing.extend_from_slice(b"pnp\ntcp\n");
            Ok(listing)
        }
        ProcEntryType::NetPnp => net::generate_pnp().map(String::into_bytes),
        ProcEntryType::NetTcp => crate::net::tcp::proc_net_tcp().map(String::into_bytes),
        ProcEntryType::ConsoleDir => text(String::from("check\n")),
        ProcEntryType::ConsoleCheck => crate::console::proc_console_check().map(String::into_bytes),
        #[cfg(target_arch = "x86_64")]
        ProcEntryType::FbconDir => text(String::from("check\n")),
        #[cfg(target_arch = "x86_64")]
        ProcEntryType::FbconCheck => crate::console::proc_fbcon_check().map(String::into_bytes),
        ProcEntryType::BreenixDir => {
            // Directory listing
            text(String::from("testing\next2\nframes\nfilesystems\n"))
        }
        ProcEntryType::BreenixTesting => {
            #[cfg(feature = "testing")]
            {
                text(String::from("1\n"))
            }
            #[cfg(not(feature = "testing"))]
            {
                text(String::from("0\n"))
            }
        }
        ProcEntryType::PidDir(pid) => text(generate_pid_dir(pid)),
        ProcEntryType::PidStatus(pid) => text(generate_pid_status(pid)),
        ProcEntryType::PidStat(pid) => generate_pid_stat(pid),
        ProcEntryType::PidCmdline(pid) => generate_pid_cmdline(pid),
        ProcEntryType::PidTasks(pid) => {
            let mut listing = String::new();
            for tid in thread_ids(pid)? {
                listing.push_str(&alloc::format!("{tid}\n"));
            }
            text(listing)
        }
        ProcEntryType::PidTask(_, _) => text(String::from("kernel_output\n")),
        ProcEntryType::PidTaskKernelOutput(pid, tid) => {
            crate::process::privilege::require_inspect(pid).map_err(|errno| -(errno as i32))?;
            text(alloc::format!("{}\n", thread_kernel_output(pid, tid).ok_or(-2)?))
        }
        // Opening a link without following it (O_NOFOLLOW), as on Linux.
        ProcEntryType::PidExe(_) | ProcEntryType::SelfLink => Err(-40),
    }
}

use crate::process::privilege::Operation;

/// `privilege::require`, with procfs's negative errno.
fn require(operation: Operation) -> Result<(), i32> {
    crate::process::privilege::require(operation).map_err(|errno| -(errno as i32))
}

/// A text entry's content as bytes.
fn text(content: String) -> Result<Vec<u8>, i32> {
    Ok(content.into_bytes())
}

/// Read from a procfs file by path
///
/// # Arguments
/// * `path` - Full path like "/proc/uptime" or relative like "uptime"
///
/// # Returns
/// The content's bytes, or an error code
pub fn read_file(path: &str) -> Result<Vec<u8>, i32> {
    // Try full path first
    if let Some(entry) = lookup_by_path(path) {
        return read_entry(entry.entry_type);
    }

    // Try relative path
    let relative = path.trim_start_matches("/proc/").trim_start_matches('/');
    if let Some(entry) = lookup(relative) {
        return read_entry(entry.entry_type);
    }

    Err(-2) // ENOENT
}

// =============================================================================
// Content Generators for Standard Entries
// =============================================================================

/// Settle queued orphan reclamation before snapshotting allocation counters
/// and their persisted counterparts. Failures reach the reader.
fn generate_ext2() -> Result<String, i32> {
    use crate::fs::ext2;
    use core::fmt::Write;
    let mut text = String::new();
    for home in [false, true] {
        let guard = ext2::accounting_fs_write(home).map_err(|_| 5)?;
        let name = if home { "home" } else { "root" };
        if let Some(fs) = guard.as_ref() {
            // Read from the device itself, not the mount's copy of what it wrote.
            let (disk, groups) = fs.persisted_metadata().map_err(|_| 5)?;
            let free = fs.superblock.s_free_blocks_count;
            let disk_free = disk.s_free_blocks_count;
            let group_free: u64 = fs.block_groups.iter().map(|g| g.bg_free_blocks_count as u64).sum();
            let disk_group_free: u64 = groups.iter().map(|g| g.bg_free_blocks_count as u64).sum();
            writeln!(text, "{} {} {} {} {} {}", name, fs.superblock.block_size(),
                free, disk_free, group_free, disk_group_free).map_err(|_| 5)?;
        }
    }
    Ok(text)
}

/// Generate /proc/uptime content
fn generate_uptime() -> String {
    use alloc::format;

    // Use the boot clock, independent of each architecture's timer frequency.
    let (uptime_secs, nanos) = crate::time::get_monotonic_time_ns();
    let uptime_frac = nanos / 10_000_000; // Two decimal places

    // Format: uptime_seconds idle_seconds
    // For now, idle is always 0 since we don't track it
    format!("{}.{:02} 0.00\n", uptime_secs, uptime_frac)
}

/// Generate /proc/version content
fn generate_version() -> String {
    use alloc::format;

    format!(
        "Breenix version {} ({}) (rustc {}) #{}\n",
        env!("CARGO_PKG_VERSION"),
        env!("CARGO_PKG_NAME"),
        "nightly", // Could be made more specific
        1          // Build number placeholder
    )
}

/// Generate /proc/breenix/frames: the frame census, then what became of process
/// page tables (`teardown` counters), the reclaim receipts still queued, and
/// whether boot's own work is over (`boot::settled`).
fn generate_frames() -> Result<String, i32> {
    use crate::tracing::providers::teardown;
    use core::fmt::Write;

    let mut text = crate::memory::frame_allocator::frame_census()?;
    let (pending, parked) = crate::task::process_task::reclaim_queue_lengths();
    let _ = write!(
        text,
        "tables recorded={} returned={} lost={}\n\
         roots retired={} no_proof={} no_arch={} terminated={} undecided={} mid_retire={}\n\
         receipts pending={} parked={}\n\
         boot settled={}\n",
        teardown::PT_TABLE_FRAMES_RECORDED.aggregate(),
        teardown::PT_TABLE_FRAMES_RETURNED.aggregate(),
        teardown::PT_RETIRE_FRAMES_LOST.aggregate(),
        teardown::PT_ROOTS_RETIRED.aggregate(),
        teardown::PT_ROOT_ABANDONED_NO_PROOF.aggregate(),
        teardown::PT_ROOT_ABANDONED_NO_ARCH.aggregate(),
        teardown::PT_ROOT_ABANDONED_TERMINATED.aggregate(),
        teardown::PT_ROOT_DROPPED_UNDECIDED.aggregate(),
        teardown::PT_ROOT_DROPPED_MID_RETIRE.aggregate(),
        pending,
        parked,
        crate::boot::settled::settled() as u8
    );
    Ok(text)
}

/// Generate /proc/meminfo content
fn generate_meminfo() -> String {
    use crate::memory::slab::{FD_TABLE_SLAB, SIGNAL_HANDLERS_SLAB};
    use alloc::format;

    let stats = crate::memory::frame_allocator::memory_stats();

    let total_kb = stats.total_bytes / 1024;
    // Used frames = allocated minus those returned to the free list
    let used_frames = if stats.allocated_frames > stats.free_list_frames {
        stats.allocated_frames - stats.free_list_frames
    } else {
        0
    };
    let used_kb = (used_frames as u64) * 4096 / 1024;
    let free_kb = if total_kb > used_kb {
        total_kb - used_kb
    } else {
        0
    };

    // Slab memory usage: sum of allocated objects * object size for each slab
    let slab_kb = {
        let fd_stats = FD_TABLE_SLAB.stats();
        let sig_stats = SIGNAL_HANDLERS_SLAB.stats();
        let fd_bytes = (fd_stats.allocated * fd_stats.obj_size) as u64;
        let sig_bytes = (sig_stats.allocated * sig_stats.obj_size) as u64;
        (fd_bytes + sig_bytes) / 1024
    };

    // Kernel heap is 32 MiB fixed
    let kernel_stack_kb: u64 = 32 * 1024; // 32 MiB heap as rough kernel memory estimate

    format!(
        "MemTotal:       {:>8} kB\n\
         MemFree:        {:>8} kB\n\
         MemAvailable:   {:>8} kB\n\
         Buffers:        {:>8} kB\n\
         Cached:         {:>8} kB\n\
         SwapCached:     {:>8} kB\n\
         Active:         {:>8} kB\n\
         Inactive:       {:>8} kB\n\
         SwapTotal:      {:>8} kB\n\
         SwapFree:       {:>8} kB\n\
         Slab:           {:>8} kB\n\
         KernelStack:    {:>8} kB\n\
         PageTables:     {:>8} kB\n\
         CommitLimit:    {:>8} kB\n\
         Committed_AS:   {:>8} kB\n\
         VmallocTotal:   {:>8} kB\n\
         VmallocUsed:    {:>8} kB\n\
         KernelHeapFree: {:>8} kB\n\
         KernelHeapPeak: {:>8} kB\n",
        total_kb,
        free_kb,
        free_kb, // Available ~= free (no page cache pressure)
        0u64,    // No buffer cache
        0u64,    // No page cache
        0u64,    // No swap cache
        0u64,    // No active/inactive tracking
        0u64,    // No active/inactive tracking
        0u64,    // No swap
        0u64,    // No swap
        slab_kb,
        kernel_stack_kb,
        0u64,     // Page tables not tracked yet
        total_kb, // CommitLimit = total (no overcommit)
        used_kb,  // Committed_AS = used memory
        0u64,     // No vmalloc tracking
        0u64,     // No vmalloc tracking
        crate::memory::heap::free_bytes() / 1024,
        crate::memory::heap::high_water_bytes() / 1024,
    )
}

/// Generate /proc/cpuinfo content
fn generate_cpuinfo() -> String {
    use alloc::format;

    #[cfg(target_arch = "x86_64")]
    {
        if let Some(info) = crate::arch_impl::x86_64::cpuinfo::get() {
            // Get real TSC frequency for cpu MHz and bogomips
            let freq_hz = crate::arch_impl::x86_64::timer::frequency_hz();
            let mhz = freq_hz / 1_000_000;
            let mhz_frac = (freq_hz % 1_000_000) / 1_000; // 3 decimal places
                                                          // Linux computes bogomips as 2 * (tsc_freq / 1_000_000)
            let bogomips_int = (freq_hz * 2) / 1_000_000;
            let bogomips_frac = ((freq_hz * 2) % 1_000_000) / 10_000; // 2 decimal places

            let cache_kb = info.cache_size_kb();
            let online = crate::arch_impl::x86_64::smp::cpus_online().max(1) as usize;

            let mut output = String::new();
            for cpu in 0..online {
                use core::fmt::Write;
                let _ = write!(
                    output,
                    "processor\t: {}\n\
                     vendor_id\t: {}\n\
                     cpu family\t: {}\n\
                     model\t\t: {}\n\
                     model name\t: {}\n\
                     stepping\t: {}\n\
                     cpu MHz\t\t: {}.{:03}\n\
                     cache size\t: {} KB\n\
                     physical id\t: 0\n\
                     siblings\t: {}\n\
                     core id\t\t: {}\n\
                     apicid\t\t: {}\n\
                     cpu cores\t: {}\n\
                     fpu\t\t: {}\n\
                     fpu_exception\t: {}\n\
                     cpuid level\t: {}\n\
                     clflush size\t: {}\n\
                     flags\t\t: {}\n\
                     bogomips\t: {}.{:02}\n\n",
                    cpu,
                    info.vendor_str(),
                    info.family,
                    info.model,
                    info.brand_str(),
                    info.stepping,
                    mhz,
                    mhz_frac,
                    cache_kb,
                    online,
                    cpu,
                    crate::arch_impl::x86_64::smp::cpu_apic_id(cpu).unwrap_or(0),
                    online,
                    if info.features_edx & 1 != 0 {
                        "yes"
                    } else {
                        "no"
                    },
                    if info.features_edx & 1 != 0 {
                        "yes"
                    } else {
                        "no"
                    },
                    info.max_leaf,
                    info.clflush_size,
                    info.flags_string(),
                    bogomips_int,
                    bogomips_frac,
                );
            }
            output
        } else {
            format!("processor\t: 0\nmodel name\t: Unknown (CPUID not initialized)\n\n")
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        if let Some(info) = crate::arch_impl::aarch64::cpuinfo::get() {
            let part_name = info.part_name();
            let impl_name = info.implementer_name();
            let model_name = if part_name != "Unknown" {
                format!("{} {}", impl_name, part_name)
            } else {
                format!("{} (part 0x{:03x})", impl_name, info.part_number())
            };

            // Get counter frequency for BogoMIPS (Linux ARM64: counter_freq * 2 / 1_000_000)
            let counter_freq = crate::arch_impl::aarch64::timer::frequency_hz();
            let bogomips_int = (counter_freq * 2) / 1_000_000;
            let bogomips_frac = ((counter_freq * 2) % 1_000_000) / 10_000;

            let num_cpus = crate::arch_impl::aarch64::smp::cpus_online() as usize;
            let num_cpus = if num_cpus == 0 { 1 } else { num_cpus };

            let mut output = String::new();
            for cpu_id in 0..num_cpus {
                use core::fmt::Write;
                let _ = write!(
                    output,
                    "processor\t: {}\n\
                     BogoMIPS\t: {}.{:02}\n\
                     Features\t: {}\n\
                     CPU implementer\t: 0x{:02x}\n\
                     CPU architecture\t: 8\n\
                     CPU variant\t: 0x{:x}\n\
                     CPU part\t: 0x{:03x}\n\
                     CPU revision\t: {}\n\
                     Model name\t: {}\n\
                     Address sizes\t: {} bits physical\n\n",
                    cpu_id,
                    bogomips_int,
                    bogomips_frac,
                    info.features_string(),
                    info.implementer(),
                    info.variant(),
                    info.part_number(),
                    info.revision(),
                    model_name,
                    info.pa_bits(),
                );
            }
            output
        } else {
            format!("processor\t: 0\nmodel name\t: Unknown (CPU detection not initialized)\n\n")
        }
    }
}

/// Generate /proc/slabinfo content
fn generate_slabinfo() -> String {
    use crate::memory::slab::{FD_TABLE_SLAB, SIGNAL_HANDLERS_SLAB};
    use alloc::format;

    let mut out = String::from("# name            active   free  total  objsize  pct\n");
    for slab in &[&FD_TABLE_SLAB, &SIGNAL_HANDLERS_SLAB] {
        let s = slab.stats();
        let pct = if s.capacity > 0 {
            (s.allocated * 100) / s.capacity
        } else {
            0
        };
        out.push_str(&format!(
            "{:<16}  {:>5}  {:>5}  {:>5}  {:>7}  {:>2}%\n",
            s.name, s.allocated, s.free, s.capacity, s.obj_size, pct,
        ));
    }
    out
}

/// Generate /proc/stat content (kernel activity counters)
fn generate_stat() -> String {
    use crate::tracing::providers::counters::{
        COW_FAULT_TOTAL, CTX_SWITCH_TOTAL, EXEC_TOTAL, FORK_TOTAL, GPU_BYTES_UPLOADED,
        GPU_FULL_UPLOADS, GPU_PARTIAL_UPLOADS, IDLE_TICK_TOTAL, IRQ_TOTAL, SYSCALL_TOTAL,
        TIMER_TICK_TOTAL,
    };
    use core::fmt::Write;

    let mut out = String::with_capacity(512);

    // Per-CPU lines: "cpuN <timer_ticks> <idle_ticks>"
    let num_cpus = procfs_online_cpus();

    for cpu in 0..num_cpus {
        let ticks = procfs_cpu_ticks(cpu);
        let idle = IDLE_TICK_TOTAL.get_cpu(cpu);
        let _ = write!(out, "cpu{} {} {}\n", cpu, ticks, idle);
    }

    let (cpu_online, cpu_sample_ticks, cpu_capacity_ticks) = procfs_cpu_accounting_ticks();

    // Aggregate counters (existing format, backwards compatible)
    let _ = write!(
        out,
        "syscalls {}\n\
         interrupts {}\n\
         context_switches {}\n\
         timer_ticks {}\n\
         global_ticks {}\n\
         ms_per_tick {}\n\
         cpu_online {}\n\
         cpu_sample_ticks {}\n\
         cpu_capacity_ticks {}\n\
         forks {}\n\
         execs {}\n\
         cow_faults {}\n\
         gpu_bytes {}\n\
         gpu_full {}\n\
         gpu_partial {}\n",
        SYSCALL_TOTAL.aggregate(),
        IRQ_TOTAL.aggregate(),
        CTX_SWITCH_TOTAL.aggregate(),
        TIMER_TICK_TOTAL.aggregate(),
        crate::time::get_ticks(),
        crate::time::timer::MS_PER_TICK,
        cpu_online,
        cpu_sample_ticks,
        cpu_capacity_ticks,
        FORK_TOTAL.aggregate(),
        EXEC_TOTAL.aggregate(),
        COW_FAULT_TOTAL.aggregate(),
        GPU_BYTES_UPLOADED.aggregate(),
        GPU_FULL_UPLOADS.aggregate(),
        GPU_PARTIAL_UPLOADS.aggregate(),
    );
    #[cfg(target_arch = "aarch64")]
    {
        let _ = write!(
            out,
            "net_msi_irqs {}\n",
            crate::drivers::virtio::net_pci::msi_interrupt_count()
        );
    }
    out
}

fn procfs_online_cpus() -> usize {
    #[cfg(target_arch = "aarch64")]
    let num_cpus = crate::arch_impl::aarch64::smp::cpus_online() as usize;
    #[cfg(target_arch = "x86_64")]
    let num_cpus = crate::arch_impl::x86_64::smp::cpus_online() as usize;

    if num_cpus == 0 {
        1
    } else {
        num_cpus
    }
}

/// Ticks `cpu` has run for, the total its idle ticks are part of. Each CPU is
/// credited with the ticks since its previous credit -- on aarch64 by its timer
/// interrupt, on x86_64 on each interrupt return through the scheduler -- so
/// the count keeps up with the global tick when interrupts are late.
fn procfs_cpu_ticks(cpu: usize) -> u64 {
    #[cfg(target_arch = "aarch64")]
    {
        crate::arch_impl::aarch64::timer_interrupt::cpu_elapsed_ticks(cpu)
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        crate::arch_impl::x86_64::smp::cpu_elapsed_ticks(cpu)
    }
}

fn procfs_cpu_accounting_ticks() -> (usize, u64, u64) {
    let cpu_online = procfs_online_cpus();
    let capacity_ticks = (0..cpu_online).map(procfs_cpu_ticks).sum::<u64>();
    let sample_ticks = capacity_ticks / cpu_online as u64;

    (cpu_online, sample_ticks, capacity_ticks)
}

/// Generate /proc/cowinfo content (copy-on-write statistics)
fn generate_cowinfo() -> String {
    use alloc::format;

    let stats = crate::memory::cow_stats::get_stats();

    format!(
        "total_faults {}\n\
         pages_copied {}\n\
         sole_owner_optimizations {}\n\
         manager_path {}\n\
         direct_path {}\n",
        stats.total_faults,
        stats.pages_copied,
        stats.sole_owner_opt,
        stats.manager_path,
        stats.direct_path,
    )
}

/// Generate /proc/mounts content (mounted filesystems)
fn generate_mounts() -> String {
    use alloc::format;

    let mounts = crate::fs::vfs::mount::list_mounts();
    let mut out = String::new();

    for (_mount_id, mount_path, fs_type, flags) in &mounts {
        out.push_str(&format!("none {} {} {} 0 0\n", mount_path, fs_type, flags.options()));
    }

    // If no mounts are registered yet, return an empty string
    out
}

/// Generate /proc/pids content (newline-separated PID list)
fn generate_pids() -> String {
    use alloc::format;

    let manager_guard = crate::process::manager();
    let mut out = String::new();
    if let Some(ref manager) = *manager_guard {
        let mut pids = manager.all_pids();
        pids.sort();
        for pid in pids {
            out.push_str(&format!("{}\n", pid.as_u64()));
        }
    }
    out
}

/// Generate /proc/kmsg content (kernel log ring buffer)
fn generate_kmsg() -> String {
    crate::log_buffer::read_all()
}

// =============================================================================
// Dynamic Per-PID Entries
// =============================================================================

/// Check if a PID exists in the process manager.
///
/// This acquires the process manager lock, so callers must NOT hold the PROCFS lock.
fn pid_exists(pid: u64) -> bool {
    use crate::process::ProcessId;

    let manager_guard = crate::process::manager();
    if let Some(ref manager) = *manager_guard {
        manager.get_process(ProcessId::new(pid)).is_some()
    } else {
        false
    }
}

/// Try to resolve a full /proc path as a dynamic per-PID entry.
///
/// Handles:
/// - `/proc/123` -> PidDir(123)
/// - `/proc/123/status` -> PidStatus(123)
///
/// Returns None if the path doesn't match a PID pattern or the PID doesn't exist.
/// This function acquires the process manager lock, so callers must NOT hold the
/// PROCFS lock (to avoid lock ordering issues).
fn lookup_pid_path(path: &str) -> Option<ProcEntry> {
    let (pid, entry) = pid_path::parse(path)?;
    let pid = match pid {
        pid_path::Pid::Caller => current_pid()?,
        pid_path::Pid::Number(pid) => pid,
    };
    if !pid_exists(pid) {
        return None;
    }
    let in_group = |tid: u64| thread_kernel_output(pid, tid).is_some();
    match entry {
        pid_path::Entry::Task(tid) | pid_path::Entry::KernelOutput(tid) if !in_group(tid) => return None,
        _ => {}
    }
    Some(ProcEntry::new(match entry {
        pid_path::Entry::Tasks => ProcEntryType::PidTasks(pid),
        pid_path::Entry::Task(tid) => ProcEntryType::PidTask(pid, tid),
        pid_path::Entry::KernelOutput(tid) => ProcEntryType::PidTaskKernelOutput(pid, tid),
        pid_path::Entry::Directory => ProcEntryType::PidDir(pid),
        pid_path::Entry::Status => ProcEntryType::PidStatus(pid),
        pid_path::Entry::Exe => ProcEntryType::PidExe(pid),
        pid_path::Entry::Stat => ProcEntryType::PidStat(pid),
        pid_path::Entry::Cmdline => ProcEntryType::PidCmdline(pid),
    }))
}

/// The thread ids of thread group `pid`, in increasing order.
pub fn thread_ids(pid: u64) -> Result<Vec<u64>, i32> {
    let mut ids = Vec::new();
    let manager_guard = crate::process::manager();
    if let Some(ref manager) = *manager_guard {
        for (row, process) in manager.iter_processes() {
            if process.thread_group_id.unwrap_or(row.as_u64()) != pid || process.is_terminated() {
                continue;
            }
            if let Some(thread) = process.main_thread.as_ref() {
                ids.try_reserve(1).map_err(|_| -12)?;
                ids.push(thread.id);
            }
        }
    }
    ids.sort_unstable();
    Ok(ids)
}

/// The kernel output bytes of thread `tid` (docs/gui/breengel.md section 22), if it is a live
/// thread of thread group `pid`.
pub fn thread_kernel_output(pid: u64, tid: u64) -> Option<u64> {
    let manager_guard = crate::process::manager();
    let manager = manager_guard.as_ref()?;
    let (row, process) = manager.find_process_by_thread(tid)?;
    if process.thread_group_id.unwrap_or(row.as_u64()) != pid || process.is_terminated() {
        return None;
    }
    let thread = process.main_thread.as_ref().filter(|t| t.id == tid)?;
    Some(thread.signals.kernel_output_bytes.load(core::sync::atomic::Ordering::Relaxed))
}

fn lookup_trace_teardown_path(path: &str) -> Option<ProcEntry> {
    let relative = path.trim_start_matches('/');
    let pid_text = relative.strip_prefix("trace/teardown/")?;
    if pid_text.is_empty()
        || pid_text.contains('/')
        || (pid_text.len() > 1 && pid_text.starts_with('0'))
    {
        return None;
    }
    let pid = pid_text.parse().ok()?;
    Some(ProcEntry::new(ProcEntryType::TraceTeardownPid(pid)))
}

/// Generate directory listing for /proc/[pid]
///
/// Lists the available per-process files.
fn generate_pid_dir(pid: u64) -> String {
    use alloc::format;

    if !pid_exists(pid) {
        return format!("Process {} not found\n", pid);
    }

    let mut listing = String::new();
    for name in PID_DIR_ENTRIES {
        listing.push_str(name);
        listing.push('\n');
    }
    listing
}

/// Generate /proc/[pid]/status content
///
/// Formatted similarly to Linux's /proc/[pid]/status:
/// ```text
/// Name:   init_shell
/// Pid:    3
/// PPid:   1
/// State:  Running
/// Uid:    0   0   0   0
/// Gid:    0   0   0   0
/// Children:   4 5
/// FdCount:    6
/// VmSize: 2048 kB
/// VmCode: 8 kB
/// VmHeap: 64 kB
/// VmStack:    16 kB
/// VmRSS:      120 kB
/// ```
fn generate_pid_status(pid: u64) -> String {
    use crate::process::ProcessId;
    use crate::process::ProcessState;
    use alloc::format;

    // Charge in-flight time before acquiring PROCESS_MANAGER: scheduler is
    // Level 1 and process manager Level 2. The shared account retains exits.
    crate::task::scheduler::charge_process_cpu(pid);
    let (cpu_online, cpu_sample_ticks, cpu_capacity_ticks) = procfs_cpu_accounting_ticks();
    let scheduler_state = crate::task::scheduler::get_process_display_state(pid);

    // VmRSS walks the process's tables: a fork copying them is waited for
    // with the lock released (`memory::fork_copy`).
    let manager_guard = crate::process::manager_once_copied(|manager| {
        manager
            .get_process(ProcessId::new(pid))
            .and_then(|process| process.cr3_value())
    });
    let manager = match manager_guard.as_ref() {
        Some(m) => m,
        None => return String::from("Process manager not available\n"),
    };

    let process = match manager.get_process(ProcessId::new(pid)) {
        Some(p) => p,
        None => return format!("Process {} not found\n", pid),
    };

    let state_str = scheduler_state.unwrap_or(match process.state {
        ProcessState::Creating => "Creating",
        ProcessState::Ready => "Ready",
        ProcessState::Running => "Running",
        ProcessState::Blocked => "Blocked",
        ProcessState::Terminated(_) => "Terminated",
    });

    // An exited row remains visible until its parent reaps it. Linux procfs
    // identifies that state as a zombie, independently of scheduler ownership.
    let state_str = if state_str == "Terminated" {
        "Z (zombie)"
    } else {
        state_str
    };

    let ppid = match process.parent {
        Some(parent) => parent.as_u64(),
        None => 0,
    };

    // Format children list
    let children_str = if process.children.is_empty() {
        String::new()
    } else {
        let child_strs: Vec<String> = process
            .children
            .iter()
            .map(|c| format!("{}", c.as_u64()))
            .collect();
        child_strs.join(" ")
    };

    // Count open file descriptors
    let fd_count = process.fd_table.open_fd_count();

    // Memory sizes in kB
    let vm_code_kb = process.memory_usage.code_size / 1024;
    let vm_heap_kb = process.memory_usage.heap_size / 1024;
    let vm_stack_kb = process.memory_usage.stack_size / 1024;
    // Resident user pages, counted from the page table the process runs on.
    // User mappings are 4 KiB leaves; kernel entries carry no user access.
    let vm_rss_kb = process.page_table.as_ref().map_or(0, |table| {
        #[cfg(not(target_arch = "x86_64"))]
        use crate::memory::arch_stub::PageTableFlags;
        #[cfg(target_arch = "x86_64")]
        use x86_64::structures::paging::PageTableFlags;
        let mut resident = 0u64;
        let _ = table.walk_mapped_pages(|_, _, flags| {
            if flags.contains(PageTableFlags::USER_ACCESSIBLE) {
                resident += 1;
            }
        });
        resident * 4
    });
    let (user_ns, system_ns) = process.cpu.split_ns();
    let cpu_time_us = user_ns / 1_000 + system_ns / 1_000;

    format!(
        "Name:\t{}\n\
         Pid:\t{}\n\
         PPid:\t{}\n\
         State:\t{}\n\
         Uid:\t{}\t{}\t{}\t{}\n\
         Gid:\t{}\t{}\t{}\t{}\n\
         Children:\t{}\n\
         FdCount:\t{}\n\
         VmSize:\t{} kB\n\
         VmCode:\t{} kB\n\
         VmHeap:\t{} kB\n\
         VmStack:\t{} kB\n\
         VmLck:\t{} kB\n\
         VmRSS:\t{} kB\n\
         CpuTicks:\t{}\n\
         CpuTimeUs:\t{}\n\
         CpuSampleTicks:\t{}\n\
         CpuCapacityTicks:\t{}\n\
         CpuOnline:\t{}\n",
        pid_path::command_name(&process.name),
        pid,
        ppid,
        state_str,
        // Real, effective, saved and file-system IDs; the file-system ID
        // is the effective one.
        process.cred.uid,
        process.cred.euid,
        process.cred.suid,
        process.cred.euid,
        process.cred.gid,
        process.cred.egid,
        process.cred.sgid,
        process.cred.egid,
        children_str,
        fd_count,
        // The bytes mapped as RLIMIT_AS counts them, from the address space's owner row.
        manager
            .find_process_by_cr3(process.inherited_cr3.unwrap_or_else(|| {
                process.page_table.as_ref().map_or(0, |t| t.level_4_frame().start_address().as_u64())
            }))
            .map_or(process.mapped_bytes(), |(_, owner)| owner.mapped_bytes())
            / 1024,
        vm_code_kb,
        vm_heap_kb,
        vm_stack_kb,
        manager.find_process_by_cr3(process.inherited_cr3.unwrap_or_else(|| {
            process.page_table.as_ref().map_or(0, |t| t.level_4_frame().start_address().as_u64())
        })).map_or(0, |(_, owner)| owner.memory_locks.bytes() / 1024),
        vm_rss_kb,
        process.cpu.ticks(),
        cpu_time_us,
        cpu_sample_ticks,
        cpu_capacity_ticks,
        cpu_online,
    )
}

/// Generate /proc/[pid]/stat: the process in Linux's one-line layout
/// (proc_pid_stat(5)), which `ps` reads. ENOENT once the process is gone.
fn generate_pid_stat(pid: u64) -> Result<Vec<u8>, i32> {
    use crate::process::{ProcessId, ProcessState};
    const ENOENT: i32 = 2;
    const ENOMEM: i32 = 12;

    crate::task::scheduler::charge_process_cpu(pid);
    let scheduler_state = crate::task::scheduler::get_process_display_state(pid);

    // The resident-page walk reads the process's tables: wait out a fork
    // copying them with the lock released, as /proc/[pid]/status does.
    let manager_guard = crate::process::manager_once_copied(|manager| {
        manager
            .get_process(ProcessId::new(pid))
            .and_then(|process| process.cr3_value())
    });
    let manager = manager_guard.as_ref().ok_or(ENOENT)?;
    let process = manager.get_process(ProcessId::new(pid)).ok_or(ENOENT)?;

    let mut comm = [0u8; 15];
    let name = pid_path::command_name(&process.name);
    comm[..name.len()].copy_from_slice(name.as_bytes());
    let comm_len = name.len();

    let state = if process.is_terminated() {
        b'Z'
    } else if process.job.stopped.is_some() {
        b'T'
    } else {
        match scheduler_state {
            Some("Running") | Some("Ready") => b'R',
            Some("Disk sleep") => b'D',
            Some("Blocked") => b'S',
            _ => match process.state {
                ProcessState::Blocked => b'S',
                _ => b'R',
            },
        }
    };
    let group = process.thread_group_id.unwrap_or(pid);
    let num_threads = manager
        .iter_processes()
        .filter(|(id, row)| !row.is_terminated() && row.thread_group_id.unwrap_or(id.as_u64()) == group)
        .count() as u64;
    let rss_pages = process.page_table.as_ref().map_or(0, |table| {
        #[cfg(not(target_arch = "x86_64"))]
        use crate::memory::arch_stub::PageTableFlags;
        #[cfg(target_arch = "x86_64")]
        use x86_64::structures::paging::PageTableFlags;
        let mut resident = 0u64;
        let _ = table.walk_mapped_pages(|_, _, flags| {
            if flags.contains(PageTableFlags::USER_ACCESSIBLE) {
                resident += 1;
            }
        });
        resident
    });
    let usage = &process.memory_usage;
    let vsize = process
        .vmas
        .iter()
        .map(|vma| vma.end.as_u64().saturating_sub(vma.start.as_u64()))
        .fold(
            process
                .image_size
                .saturating_add((usage.heap_size + usage.stack_size) as u64),
            u64::saturating_add,
        );
    let signals = &process.signals;
    let (mut ignored, mut caught) = (0u64, 0u64);
    for sig in 1..=crate::signal::constants::NSIG {
        let action = signals.get_handler(sig);
        if action.is_ignore() {
            ignored |= 1 << (sig - 1);
        } else if action.is_handler() {
            caught |= 1 << (sig - 1);
        }
    }
    let sched = process.main_thread.as_ref().map(|thread| thread.sched);
    let (utime_ns, stime_ns) = process.cpu.split_ns();
    let (cutime_ns, cstime_ns) = process.cpu.children_split_ns();
    let ctty = process.ctty;
    let sid = process.sid;
    let mut stat = pid_stat::PidStat {
        pid,
        comm: "",
        state,
        ppid: process.parent.map_or(0, |parent| parent.as_u64()),
        pgrp: process.pgid.as_u64(),
        session: sid.as_u64(),
        tty_nr: match ctty {
            // tty1 is the console, which processes reach as /dev/console.
            Some(crate::tty::ctty::TtyId::Vt(1)) => pid_stat::tty_nr(5, 1),
            Some(crate::tty::ctty::TtyId::Vt(n)) => pid_stat::tty_nr(4, u32::from(n)),
            Some(crate::tty::ctty::TtyId::Pty(n)) => pid_stat::tty_nr(136, n),
            None => 0,
        },
        tpgid: -1,
        utime_ns,
        stime_ns,
        cutime_ns,
        cstime_ns,
        nice: process.nice,
        num_threads,
        start_ms: process.start_ms,
        vsize,
        rss_pages,
        pending: signals.pending_set(),
        blocked: signals.blocked(),
        ignored,
        caught,
        rt_priority: sched.map_or(0, |sched| sched.priority as u32),
        policy: sched
            .map(|sched| sched.policy)
            .filter(|&policy| policy != crate::task::thread::SchedPolicy::KERNEL)
            .map_or(0, u32::from),
        exit_code: process
            .exit_code
            .filter(|_| process.is_terminated())
            .map_or(0, pid_stat::exit_status),
    };
    drop(manager_guard);

    // The terminal's foreground group, read with no process lock held.
    stat.tpgid = ctty
        .and_then(crate::tty::ctty::TtyRef::resolve)
        .filter(|tty| tty.session() == Some(sid))
        .and_then(|tty| tty.fg_pgrp())
        .map_or(-1, |pgid| pgid.as_u64() as i64);
    stat.comm = core::str::from_utf8(&comm[..comm_len]).map_err(|_| ENOENT)?;

    let mut line = String::new();
    line.try_reserve_exact(pid_stat::MAX_LEN).map_err(|_| ENOMEM)?;
    pid_stat::write(&mut line, &stat).map_err(|_| ENOMEM)?;
    Ok(line.into_bytes())
}

/// Generate /proc/[pid]/cmdline: the bytes of the argument strings exec
/// wrote to the process's memory, each ending in a NUL byte, read from that
/// memory now, so a program that rewrites its arguments shows the new bytes.
/// When the last byte is no longer NUL, the program has written one title
/// over them (setproctitle), and the title ends at its first NUL, as on
/// Linux. Empty for a process the kernel built without an exec, as for a
/// Linux kernel thread.
fn generate_pid_cmdline(pid: u64) -> Result<Vec<u8>, i32> {
    const ENOENT: i32 = 2;
    const ENOMEM: i32 = 12;
    let id = crate::process::ProcessId::new(pid);
    // Size the buffer before taking the lock to read into it.
    let (area, copy) = {
        let guard = crate::process::manager();
        let process = guard
            .as_ref()
            .and_then(|manager| manager.get_process(id))
            .ok_or(ENOENT)?;
        if process.is_terminated() {
            // Linux's zombie has no address space to read the arguments from.
            return Ok(Vec::new());
        }
        (process.cmdline_area, process.cmdline.clone())
    };
    let Some(copy) = copy else {
        return Ok(Vec::new());
    };
    let mut content = Vec::new();
    let len = area.map_or(copy.len() as u64, |(_, len)| len);
    content.try_reserve_exact(len as usize).map_err(|_| ENOMEM)?;
    let read = area.is_some_and(|(addr, len)| {
        let guard = crate::process::manager();
        let Some(manager) = guard.as_ref() else {
            return false;
        };
        let Some(process) = manager.get_process(id) else {
            return false;
        };
        if process.cmdline_area != Some((addr, len)) {
            return false;
        }
        // A thread's row reads its process's address space.
        let table = process.page_table.as_deref().or_else(|| {
            process
                .thread_group_id
                .and_then(|group| manager.get_process(crate::process::ProcessId::new(group)))
                .and_then(|leader| leader.page_table.as_deref())
        });
        let Some(table) = table else {
            return false;
        };
        table.read_user_bytes(addr, len, &mut content);
        true
    });
    if !read {
        content.clear();
        content.extend_from_slice(&copy[..copy.len().min(content.capacity())]);
    }
    if content.last().is_some_and(|&byte| byte != 0) {
        if let Some(end) = content.iter().position(|&byte| byte == 0) {
            content.truncate(end);
        }
    }
    Ok(content)
}

/// The owner of a procfs entry: a process's directory and the files in it
/// belong to the process's effective user and group, as on Linux, and every
/// other entry to root.
pub fn owner(path: &str) -> (u32, u32) {
    let pid = match pid_path::parse(path) {
        Some((pid_path::Pid::Caller, _)) => current_pid(),
        Some((pid_path::Pid::Number(pid), _)) => Some(pid),
        None => None,
    };
    let Some(pid) = pid else {
        return (0, 0);
    };
    let guard = crate::process::manager();
    guard
        .as_ref()
        .and_then(|manager| manager.get_process(crate::process::ProcessId::new(pid)))
        .map_or((0, 0), |process| (process.cred.euid, process.cred.egid))
}

/// The caller's PID, as getpid reports it: its thread group's. Resolved before
/// taking any procfs lock.
fn current_pid() -> Option<u64> {
    let tid = crate::task::scheduler::current_thread_id()?;
    let guard = crate::process::manager();
    guard
        .as_ref()?
        .find_process_by_thread(tid)
        .map(|(pid, process)| process.thread_group_id.unwrap_or(pid.as_u64()))
}

/// The pathname `/proc/<pid>/exe` names: ENOENT once the process has
/// exited, and EACCES unless the caller may inspect the process
/// (`process::privilege::may_inspect`).
fn exe_target(pid: u64) -> Result<Vec<u8>, u64> {
    use crate::syscall::errno::{EACCES, ENOENT};
    let inspector = crate::process::privilege::current_identity()?;
    let exe_path = {
        let guard = crate::process::manager();
        let process = guard
            .as_ref()
            .and_then(|manager| manager.get_process(crate::process::ProcessId::new(pid)))
            .ok_or(ENOENT as u64)?;
        if process.is_terminated() {
            return Err(ENOENT as u64);
        }
        if !crate::process::privilege::may_inspect(&inspector, &process.cred) {
            return Err(EACCES as u64);
        }
        process.exe_path.clone().ok_or(ENOENT as u64)?
    };
    // An immutable Arc snapshot: exec or exit may proceed once the lock drops.
    let mut target = Vec::new();
    target
        .try_reserve_exact(exe_path.len())
        .map_err(|_| crate::syscall::errno::ENOMEM as u64)?;
    target.extend_from_slice(&exe_path);
    Ok(target)
}

/// The caller's process directory, relative to /proc, as `/proc/self` names it.
fn self_target() -> Result<Vec<u8>, u64> {
    use crate::syscall::errno::{ENOENT, ENOMEM};
    use core::fmt::Write;
    let pid = current_pid().ok_or(ENOENT as u64)?;
    let mut target = String::new();
    target.try_reserve_exact(20).map_err(|_| ENOMEM as u64)?;
    write!(target, "{}", pid).map_err(|_| ENOMEM as u64)?;
    Ok(target.into_bytes())
}

/// The target of the procfs link at `path` for the calling process, or no
/// target when `path` is not a link. Pathname resolution follows these as symlinks.
pub fn link_target(path: &str) -> Option<Result<Vec<u8>, u64>> {
    if path == "/proc/self" {
        return Some(self_target());
    }
    match pid_path::parse(path)? {
        (pid, pid_path::Entry::Exe) => {
            let pid = match pid {
                pid_path::Pid::Caller => current_pid(),
                pid_path::Pid::Number(pid) => Some(pid),
            };
            Some(pid.ok_or(crate::syscall::errno::ENOENT as u64).and_then(exe_target))
        }
        _ => None,
    }
}

/// readlink of a procfs pathname: EINVAL unless it names a link.
pub fn readlink(path: &str) -> Result<Vec<u8>, u64> {
    use crate::syscall::errno::{EINVAL, ENOENT};
    match link_target(path) {
        Some(target) => target,
        None if lookup_by_path(path).is_some() => Err(EINVAL as u64),
        None => Err(ENOENT as u64),
    }
}

/// Ownership of a procfs link for lstat: the process's effective IDs.
pub fn link_owner(path: &str) -> Option<(u64, u32, u32)> {
    let entry = lookup_by_path(path)?;
    let pid = match entry.entry_type {
        ProcEntryType::SelfLink => current_pid()?,
        ProcEntryType::PidExe(pid) => pid,
        _ => return None,
    };
    let guard = crate::process::manager();
    let process = guard.as_ref()?.get_process(crate::process::ProcessId::new(pid))?;
    Some((entry.entry_type.inode(), process.cred.euid, process.cred.egid))
}
