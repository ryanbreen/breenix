//! BXTEST format disk loader
//!
//! Reads userspace binaries from a test disk in BXTEST format.
//!
//! ## Disk Layout
//!
//! - Sector 0: Header (magic "BXTEST\0\0", version, binary count)
//! - Sectors 1-127: Entry table (64 bytes per entry)
//! - Sector 128+: Binary data
//!
//! Each entry contains:
//! - name[32]: Null-terminated binary name
//! - sector_offset: u64 - Starting sector
//! - size_bytes: u64 - Binary size in bytes

use alloc::vec::Vec;

/// Sector size in bytes
const SECTOR_SIZE: usize = 512;

/// BXTEST magic value
const BXTEST_MAGIC: &[u8; 8] = b"BXTEST\0\0";

/// BXTEST disk header
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TestDiskHeader {
    pub magic: [u8; 8],
    pub version: u32,
    pub binary_count: u32,
    pub reserved: [u8; 48],
}

/// Binary entry in the entry table
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BinaryEntry {
    pub name: [u8; 32],
    pub sector_offset: u64,
    pub size_bytes: u64,
    pub reserved: [u8; 16],
}

impl BinaryEntry {
    /// Get the name as a string (trimmed of null bytes and spaces)
    pub fn name_str(&self) -> &str {
        let end = self
            .name
            .iter()
            .position(|&b| b == 0 || b == b' ')
            .unwrap_or(self.name.len());
        core::str::from_utf8(&self.name[..end]).unwrap_or("")
    }
}

/// Test disk reader
pub struct TestDisk {
    pub header: TestDiskHeader,
    pub entries: Vec<BinaryEntry>,
}

impl TestDisk {
    /// Read and parse a test disk from the VirtIO block device
    pub fn read() -> Result<Self, &'static str> {
        use crate::drivers::virtio::block_mmio::read_sector;

        crate::serial_println!("[test_disk] Reading BXTEST disk...");

        // Read sector 0 (header)
        let mut sector0 = [0u8; SECTOR_SIZE];
        read_sector(0, 0, &mut sector0)?;

        // Parse header
        let header: TestDiskHeader =
            unsafe { core::ptr::read_unaligned(sector0.as_ptr() as *const TestDiskHeader) };

        // Validate magic
        if &header.magic != BXTEST_MAGIC {
            crate::serial_println!("[test_disk] Invalid magic: {:?}", &header.magic[..6]);
            return Err("Invalid BXTEST magic");
        }

        crate::serial_println!(
            "[test_disk] Found BXTEST disk: version={}, {} binaries",
            header.version,
            header.binary_count
        );

        // Read entry table (sectors 1-127, 8 entries per sector)
        let mut entries = Vec::new();
        let entries_needed = header.binary_count as usize;
        let sectors_to_read = (entries_needed + 7) / 8; // 8 entries per sector

        for sector_idx in 0..sectors_to_read {
            let mut sector_data = [0u8; SECTOR_SIZE];
            read_sector(0, 1 + sector_idx as u64, &mut sector_data)?;

            // Parse up to 8 entries per sector
            for entry_idx in 0..8 {
                let global_idx = sector_idx * 8 + entry_idx;
                if global_idx >= entries_needed {
                    break;
                }

                let entry_offset = entry_idx * 64;
                let entry: BinaryEntry = unsafe {
                    core::ptr::read_unaligned(
                        sector_data.as_ptr().add(entry_offset) as *const BinaryEntry
                    )
                };

                crate::serial_println!(
                    "[test_disk]   [{}] '{}' at sector {}, {} bytes",
                    global_idx,
                    entry.name_str(),
                    entry.sector_offset,
                    entry.size_bytes
                );

                entries.push(entry);
            }
        }

        Ok(TestDisk { header, entries })
    }

    /// Find a binary by name
    pub fn find_binary(&self, name: &str) -> Option<&BinaryEntry> {
        self.entries.iter().find(|e| e.name_str() == name)
    }

    /// Read a binary's data from the disk
    pub fn read_binary(&self, entry: &BinaryEntry) -> Result<Vec<u8>, &'static str> {
        use crate::drivers::virtio::block_mmio::read_sector;

        let size = entry.size_bytes as usize;
        let sectors_needed = (size + SECTOR_SIZE - 1) / SECTOR_SIZE;

        crate::serial_println!(
            "[test_disk] Reading binary '{}': {} bytes ({} sectors) from sector {}",
            entry.name_str(),
            size,
            sectors_needed,
            entry.sector_offset
        );

        let mut data = Vec::with_capacity(sectors_needed * SECTOR_SIZE);

        for i in 0..sectors_needed {
            // Progress indicator every 32 sectors
            if i % 32 == 0 {
                crate::serial_println!("[test_disk]   Reading sector {}/{}", i, sectors_needed);
            }
            let mut sector = [0u8; SECTOR_SIZE];
            read_sector(0, entry.sector_offset + i as u64, &mut sector)?;
            data.extend_from_slice(&sector);
        }

        // Truncate to actual size
        data.truncate(size);

        crate::serial_println!(
            "[test_disk] Read {} bytes for '{}'",
            data.len(),
            entry.name_str()
        );

        Ok(data)
    }

    /// List all binary names
    pub fn list_binaries(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|e| e.name_str())
    }
}

/// Load a userspace binary from the test disk and start it through the
/// scheduler. The caller is the `init-loader` kernel thread (design §6), so
/// the process runs when a CPU dispatches it, as every user thread does.
/// On failure, returns an error string.
#[cfg(target_arch = "aarch64")]
pub fn run_userspace_from_disk(binary_name: &str) -> Result<(), &'static str> {
    use alloc::boxed::Box;
    use alloc::string::String;

    crate::serial_println!();
    crate::serial_println!("========================================");
    crate::serial_println!("  Loading userspace: {}", binary_name);
    crate::serial_println!("========================================");

    // Read the test disk
    let disk = TestDisk::read()?;

    // Find the requested binary
    let entry = disk
        .find_binary(binary_name)
        .ok_or("Binary not found on disk")?;

    // Read the binary data
    let elf_data = disk.read_binary(entry)?;

    // Verify it's an ELF file
    if elf_data.len() < 4 || &elf_data[0..4] != b"\x7fELF" {
        return Err("Not a valid ELF file");
    }

    crate::serial_println!("[boot] Creating process via process manager...");

    // Set up argv with the program name as argv[0]
    let argv: [&[u8]; 1] = [binary_name.as_bytes()];

    // Create a process using the process manager - this properly registers
    // the process and thread so fork() can find them
    let pid = {
        let mut manager_guard = crate::process::manager();
        if let Some(ref mut manager) = *manager_guard {
            let root = crate::process::credentials::ProcessCredentials::root();
            manager.create_process_with_argv(String::from(binary_name), &elf_data, &argv, &root, None)?
        } else {
            return Err("Process manager not initialized");
        }
    };

    crate::serial_println!("[boot] Created process with PID {}", pid.as_u64());

    // Get the process entry point and thread info
    let (entry_point, thread_id, user_sp) = {
        let manager_guard = crate::process::manager();
        if let Some(ref manager) = *manager_guard {
            if let Some(process) = manager.get_process(pid) {
                let entry = process.entry_point.as_u64();
                let thread = process
                    .main_thread
                    .as_ref()
                    .ok_or("Process has no main thread")?;
                let tid = thread.id;
                // Get the SP from the thread's context (points to argc on the stack)
                let sp = thread.context.sp_el0;
                (entry, tid, sp)
            } else {
                return Err("Process not found after creation");
            }
        } else {
            return Err("Process manager not available");
        }
    };

    crate::serial_println!(
        "[boot] Process ready: entry={:#x}, thread={}, sp={:#x}",
        entry_point,
        thread_id,
        user_sp
    );

    // Register the thread with the scheduler so fork() can find it
    let scheduler_thread = {
        let mut manager_guard = crate::process::manager();
        if let Some(ref mut manager) = *manager_guard {
            if let Some(process) = manager.get_process_mut(pid) {
                if let Some(thread) = &mut process.main_thread {
                    crate::serial_println!("[boot] Spawning thread {} to scheduler", thread.id);
                    let thread_id = thread.id;
                    Some((Box::new(thread.publish_to_scheduler()), thread_id))
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    };
    let (thread, thread_id) = scheduler_thread.ok_or("Process has no thread to schedule")?;
    crate::task::scheduler::spawn(thread);
    crate::serial_println!("[boot] {} started through the scheduler (thread {})", binary_name, thread_id);
    Ok(())
}

/// Stub for x86_64 - the real implementation uses different infrastructure
#[cfg(target_arch = "x86_64")]
pub fn run_userspace_from_disk(_binary_name: &str) -> Result<(), &'static str> {
    Err("Use x86_64-specific boot infrastructure")
}
