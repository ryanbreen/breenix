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
    /// /proc/slabinfo - slab allocator statistics
    SlabInfo,
    /// /proc/stat - kernel activity counters
    Stat,
    /// /proc/cowinfo - copy-on-write statistics
    CowInfo,
    /// /proc/mounts - mounted filesystems
    Mounts,
    /// /proc/[pid] - per-process directory (dynamic, not registered)
    PidDir(u64),
    /// /proc/[pid]/status - per-process status (dynamic, not registered)
    PidStatus(u64),
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
            ProcEntryType::SlabInfo => "slabinfo",
            ProcEntryType::Stat => "stat",
            ProcEntryType::CowInfo => "cowinfo",
            ProcEntryType::Mounts => "mounts",
            ProcEntryType::PidDir(_) => "pid",
            ProcEntryType::PidStatus(_) => "status",
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
            ProcEntryType::SlabInfo => "/proc/slabinfo",
            ProcEntryType::Stat => "/proc/stat",
            ProcEntryType::CowInfo => "/proc/cowinfo",
            ProcEntryType::Mounts => "/proc/mounts",
            // Dynamic entries don't have static paths
            ProcEntryType::PidDir(_) => "/proc/<pid>",
            ProcEntryType::PidStatus(_) => "/proc/<pid>/status",
        }
    }

    /// Get the inode number for this entry
    ///
    /// Dynamic PID entries use computed inodes:
    /// - PidDir(pid) -> 10000 + pid
    /// - PidStatus(pid) -> 20000 + pid
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
            ProcEntryType::SlabInfo => 5,
            ProcEntryType::Stat => 6,
            ProcEntryType::CowInfo => 7,
            ProcEntryType::Mounts => 8,
            ProcEntryType::PidDir(pid) => 10000 + pid,
            ProcEntryType::PidStatus(pid) => 20000 + pid,
        }
    }

    /// Check if this is a directory
    pub fn is_directory(&self) -> bool {
        matches!(self, ProcEntryType::TraceDir | ProcEntryType::PidDir(_))
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
    procfs.entries.push(ProcEntry::new(ProcEntryType::SlabInfo));
    procfs.entries.push(ProcEntry::new(ProcEntryType::Stat));
    procfs.entries.push(ProcEntry::new(ProcEntryType::CowInfo));
    procfs.entries.push(ProcEntry::new(ProcEntryType::Mounts));

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
    lookup_pid_path(path)
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
/// Returns static entries plus dynamic PID directories from the process manager.
pub fn list_entries() -> Vec<String> {
    use alloc::format;

    // Collect static entries under PROCFS lock
    let mut entries: Vec<String> = {
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
    };
    // PROCFS lock is released here before acquiring process manager lock

    // Add dynamic PID directories from the process manager
    let manager_guard = crate::process::manager();
    if let Some(ref manager) = *manager_guard {
        let mut pids = manager.all_pids();
        pids.sort();
        for pid in pids {
            entries.push(format!("{}", pid.as_u64()));
        }
    }

    entries
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
        ProcEntryType::SlabInfo => Ok(generate_slabinfo()),
        ProcEntryType::Stat => Ok(generate_stat()),
        ProcEntryType::CowInfo => Ok(generate_cowinfo()),
        ProcEntryType::Mounts => Ok(generate_mounts()),
        ProcEntryType::PidDir(pid) => Ok(generate_pid_dir(pid)),
        ProcEntryType::PidStatus(pid) => Ok(generate_pid_status(pid)),
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
    use crate::memory::slab::{FD_TABLE_SLAB, SIGNAL_HANDLERS_SLAB};

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
         VmallocUsed:    {:>8} kB\n",
        total_kb,
        free_kb,
        free_kb,         // Available ~= free (no page cache pressure)
        0u64,            // No buffer cache
        0u64,            // No page cache
        0u64,            // No swap cache
        0u64,            // No active/inactive tracking
        0u64,            // No active/inactive tracking
        0u64,            // No swap
        0u64,            // No swap
        slab_kb,
        kernel_stack_kb,
        0u64,            // Page tables not tracked yet
        total_kb,        // CommitLimit = total (no overcommit)
        used_kb,         // Committed_AS = used memory
        0u64,            // No vmalloc tracking
        0u64,            // No vmalloc tracking
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

            format!(
                "processor\t: 0\n\
                 vendor_id\t: {}\n\
                 cpu family\t: {}\n\
                 model\t\t: {}\n\
                 model name\t: {}\n\
                 stepping\t: {}\n\
                 cpu MHz\t\t: {}.{:03}\n\
                 cache size\t: {} KB\n\
                 physical id\t: 0\n\
                 siblings\t: {}\n\
                 core id\t\t: 0\n\
                 cpu cores\t: 1\n\
                 fpu\t\t: {}\n\
                 fpu_exception\t: {}\n\
                 cpuid level\t: {}\n\
                 clflush size\t: {}\n\
                 flags\t\t: {}\n\
                 bogomips\t: {}.{:02}\n\n",
                info.vendor_str(),
                info.family,
                info.model,
                info.brand_str(),
                info.stepping,
                mhz, mhz_frac,
                cache_kb,
                info.logical_processors,
                if info.features_edx & 1 != 0 { "yes" } else { "no" },
                if info.features_edx & 1 != 0 { "yes" } else { "no" },
                info.max_leaf,
                info.clflush_size,
                info.flags_string(),
                bogomips_int, bogomips_frac,
            )
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
                let _ = write!(output,
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
                    bogomips_int, bogomips_frac,
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
    use alloc::format;
    use crate::memory::slab::{FD_TABLE_SLAB, SIGNAL_HANDLERS_SLAB};

    let mut out = String::from("# name            active   free  total  objsize  pct\n");
    for slab in &[&FD_TABLE_SLAB, &SIGNAL_HANDLERS_SLAB] {
        let s = slab.stats();
        let pct = if s.capacity > 0 { (s.allocated * 100) / s.capacity } else { 0 };
        out.push_str(&format!(
            "{:<16}  {:>5}  {:>5}  {:>5}  {:>7}  {:>2}%\n",
            s.name, s.allocated, s.free, s.capacity, s.obj_size, pct,
        ));
    }
    out
}

/// Generate /proc/stat content (kernel activity counters)
fn generate_stat() -> String {
    use alloc::format;
    use crate::tracing::providers::counters::{
        SYSCALL_TOTAL, IRQ_TOTAL, CTX_SWITCH_TOTAL, TIMER_TICK_TOTAL,
        FORK_TOTAL, EXEC_TOTAL, COW_FAULT_TOTAL,
    };

    format!(
        "syscalls {}\n\
         interrupts {}\n\
         context_switches {}\n\
         timer_ticks {}\n\
         forks {}\n\
         execs {}\n\
         cow_faults {}\n",
        SYSCALL_TOTAL.aggregate(),
        IRQ_TOTAL.aggregate(),
        CTX_SWITCH_TOTAL.aggregate(),
        TIMER_TICK_TOTAL.aggregate(),
        FORK_TOTAL.aggregate(),
        EXEC_TOTAL.aggregate(),
        COW_FAULT_TOTAL.aggregate(),
    )
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

    for (_mount_id, mount_path, fs_type) in &mounts {
        out.push_str(&format!("none {} {} rw 0 0\n", mount_path, fs_type));
    }

    // If no mounts are registered yet, return an empty string
    out
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
    // Strip "/proc/" prefix
    let relative = path.strip_prefix("/proc/")?;
    let relative = relative.trim_end_matches('/');

    if relative.is_empty() {
        return None;
    }

    // Split into PID component and optional sub-path
    let (pid_str, sub_path) = if let Some(slash_pos) = relative.find('/') {
        (&relative[..slash_pos], Some(&relative[slash_pos + 1..]))
    } else {
        (relative, None)
    };

    // Parse PID: must be purely numeric, no leading zeros (except "0" itself)
    if pid_str.is_empty() || (pid_str.len() > 1 && pid_str.starts_with('0')) {
        return None;
    }
    let pid: u64 = pid_str.parse().ok()?;

    // Verify the PID exists (acquires process manager lock)
    if !pid_exists(pid) {
        return None;
    }

    match sub_path {
        None => Some(ProcEntry::new(ProcEntryType::PidDir(pid))),
        Some("status") => Some(ProcEntry::new(ProcEntryType::PidStatus(pid))),
        Some(_) => None, // Unknown sub-path
    }
}

/// Generate directory listing for /proc/[pid]
///
/// Lists the available per-process files.
fn generate_pid_dir(pid: u64) -> String {
    use alloc::format;

    if !pid_exists(pid) {
        return format!("Process {} not found\n", pid);
    }

    String::from("status\n")
}

/// Generate /proc/[pid]/status content
///
/// Formatted similarly to Linux's /proc/[pid]/status:
/// ```text
/// Name:   init_shell
/// Pid:    3
/// PPid:   1
/// State:  Running
/// Children:   4 5
/// FdCount:    6
/// VmCode: 8 kB
/// VmHeap: 64 kB
/// VmStack:    16 kB
/// ```
fn generate_pid_status(pid: u64) -> String {
    use alloc::format;
    use crate::process::ProcessId;
    use crate::process::ProcessState;

    let manager_guard = crate::process::manager();
    let manager = match manager_guard.as_ref() {
        Some(m) => m,
        None => return String::from("Process manager not available\n"),
    };

    let process = match manager.get_process(ProcessId::new(pid)) {
        Some(p) => p,
        None => return format!("Process {} not found\n", pid),
    };

    let state_str = match process.state {
        ProcessState::Creating => "Creating",
        ProcessState::Ready => "Ready",
        ProcessState::Running => "Running",
        ProcessState::Blocked => "Blocked",
        ProcessState::Terminated(_) => "Terminated",
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

    format!(
        "Name:\t{}\n\
         Pid:\t{}\n\
         PPid:\t{}\n\
         State:\t{}\n\
         Children:\t{}\n\
         FdCount:\t{}\n\
         VmCode:\t{} kB\n\
         VmHeap:\t{} kB\n\
         VmStack:\t{} kB\n",
        process.name,
        pid,
        ppid,
        state_str,
        children_str,
        fd_count,
        vm_code_kb,
        vm_heap_kb,
        vm_stack_kb,
    )
}
