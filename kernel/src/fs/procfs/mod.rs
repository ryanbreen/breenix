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

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

mod trace;

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
}

impl ProcEntryType {
    /// Get the entry name (without path prefix)
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
        }
    }

    /// Get the full path
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
        }
    }

    /// Get the inode number for this entry
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
        }
    }

    /// Check if this is a directory
    pub fn is_directory(&self) -> bool {
        matches!(self, ProcEntryType::TraceDir)
    }
}

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

    // Register /proc/trace directory and entries
    procfs.entries.push(ProcEntry::new(ProcEntryType::TraceDir));
    procfs.entries.push(ProcEntry::new(ProcEntryType::TraceEnable));
    procfs.entries.push(ProcEntry::new(ProcEntryType::TraceEvents));
    procfs.entries.push(ProcEntry::new(ProcEntryType::TraceBuffer));
    procfs.entries.push(ProcEntry::new(ProcEntryType::TraceCounters));
    procfs.entries.push(ProcEntry::new(ProcEntryType::TraceProviders));

    procfs.initialized = true;
    log::info!("procfs: initialized with {} entries", procfs.entries.len());

    // Register mount point
    crate::fs::vfs::mount::mount("/proc", "procfs");
}

/// Look up an entry by path (without /proc prefix)
pub fn lookup(name: &str) -> Option<ProcEntry> {
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
    None
}

/// Look up an entry by full path
pub fn lookup_by_path(path: &str) -> Option<ProcEntry> {
    let procfs = PROCFS.lock();

    for entry in &procfs.entries {
        if entry.entry_type.path() == path {
            return Some(entry.clone());
        }
    }
    None
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
pub fn list_entries() -> Vec<String> {
    let procfs = PROCFS.lock();
    procfs
        .entries
        .iter()
        .filter(|e| !matches!(
            e.entry_type,
            ProcEntryType::TraceEnable
                | ProcEntryType::TraceEvents
                | ProcEntryType::TraceBuffer
                | ProcEntryType::TraceCounters
                | ProcEntryType::TraceProviders
        ))
        .map(|e| String::from(e.entry_type.name()))
        .collect()
}

/// List entries in the /proc/trace directory
pub fn list_trace_entries() -> Vec<String> {
    let procfs = PROCFS.lock();
    procfs
        .entries
        .iter()
        .filter(|e| matches!(
            e.entry_type,
            ProcEntryType::TraceEnable
                | ProcEntryType::TraceEvents
                | ProcEntryType::TraceBuffer
                | ProcEntryType::TraceCounters
                | ProcEntryType::TraceProviders
        ))
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
pub fn read_entry(entry_type: ProcEntryType) -> Result<String, i32> {
    match entry_type {
        ProcEntryType::Uptime => Ok(generate_uptime()),
        ProcEntryType::Version => Ok(generate_version()),
        ProcEntryType::MemInfo => Ok(generate_meminfo()),
        ProcEntryType::CpuInfo => Ok(generate_cpuinfo()),
        ProcEntryType::TraceDir => {
            // Directory listing
            let entries = list_trace_entries();
            Ok(entries.join("\n") + "\n")
        }
        ProcEntryType::TraceEnable => Ok(trace::generate_enable()),
        ProcEntryType::TraceEvents => Ok(trace::generate_events()),
        ProcEntryType::TraceBuffer => Ok(trace::generate_buffer()),
        ProcEntryType::TraceCounters => Ok(trace::generate_counters()),
        ProcEntryType::TraceProviders => Ok(trace::generate_providers()),
    }
}

/// Read from a procfs file by path
///
/// # Arguments
/// * `path` - Full path like "/proc/uptime" or relative like "uptime"
///
/// # Returns
/// The content as a String, or an error code
pub fn read_file(path: &str) -> Result<String, i32> {
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

/// Generate /proc/uptime content
fn generate_uptime() -> String {
    use alloc::format;

    // Get uptime from timer subsystem
    // Timer ticks at 200 Hz (5ms per tick)
    let ticks = crate::time::get_ticks();
    let uptime_ms = ticks * 5; // Convert ticks to milliseconds
    let uptime_secs = uptime_ms / 1000;
    let uptime_frac = (uptime_ms % 1000) / 10; // Two decimal places

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

/// Generate /proc/meminfo content
fn generate_meminfo() -> String {
    use alloc::format;

    // Get memory info from allocator if available
    #[cfg(target_arch = "x86_64")]
    let (total_kb, free_kb) = {
        // Placeholder values - would query actual allocator
        (1048576u64, 524288u64) // 1GB total, 512MB free
    };

    #[cfg(target_arch = "aarch64")]
    let (total_kb, free_kb) = {
        // Placeholder values
        (1048576u64, 524288u64)
    };

    format!(
        "MemTotal:     {:>10} kB\n\
         MemFree:      {:>10} kB\n\
         MemAvailable: {:>10} kB\n\
         Buffers:      {:>10} kB\n\
         Cached:       {:>10} kB\n",
        total_kb,
        free_kb,
        free_kb, // Available ~= free for now
        0,       // No buffer cache yet
        0        // No page cache yet
    )
}

/// Generate /proc/cpuinfo content
fn generate_cpuinfo() -> String {
    use alloc::format;

    #[cfg(target_arch = "x86_64")]
    {
        format!(
            "processor\t: 0\n\
             vendor_id\t: GenuineBreenix\n\
             cpu family\t: 6\n\
             model\t\t: 0\n\
             model name\t: Breenix Virtual CPU\n\
             flags\t\t: fpu vme de pse tsc msr pae mce cx8\n\
             bogomips\t: 3000.00\n\n"
        )
    }

    #[cfg(target_arch = "aarch64")]
    {
        format!(
            "processor\t: 0\n\
             BogoMIPS\t: 100.00\n\
             Features\t: fp asimd\n\
             CPU implementer\t: 0x00\n\
             CPU architecture\t: 8\n\
             CPU variant\t: 0x0\n\
             CPU part\t: 0x000\n\
             CPU revision\t: 0\n\n"
        )
    }
}
