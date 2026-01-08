//! Test Disk Image Builder
//!
//! Creates a raw disk image containing userspace test binaries in a simple custom format.
//!
//! ## Disk Layout
//!
//! ```text
//! Sector 0:       TestDiskHeader (64 bytes)
//! Sectors 1-127:  BinaryEntry table (up to 128 entries, 8 per sector)
//! Sector 128+:    Binary data (concatenated, sector-aligned)
//! ```
//!
//! ## Format Details
//!
//! - Sector size: 512 bytes
//! - Header: Magic "BXTEST\0\0", version 1, binary count
//! - Each entry: Name (32 bytes), sector offset (u64), size (u64)
//! - Binaries are stored contiguously starting at sector 64
//! - Each binary is padded to sector boundary
//!
//! ## Usage
//!
//! ```bash
//! cargo run -p xtask -- create-test-disk
//! ```
//!
//! Reads compiled ELF binaries from `userspace/tests/*.elf` and creates
//! `target/test_binaries.img`.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use anyhow::{bail, Result};

const SECTOR_SIZE: usize = 512;
const MAGIC: &[u8; 8] = b"BXTEST\0\0";
const VERSION: u32 = 1;
const MAX_BINARIES: usize = 128;
const DATA_START_SECTOR: u64 = 128;

/// Disk header stored in sector 0
///
/// Total size: 64 bytes (fits in first part of sector 0)
#[repr(C)]
#[derive(Clone, Copy)]
struct TestDiskHeader {
    magic: [u8; 8],       // "BXTEST\0\0"
    version: u32,         // Format version (currently 1)
    binary_count: u32,    // Number of binaries on disk
    reserved: [u8; 48],   // Padding for future use
}

impl TestDiskHeader {
    fn new(binary_count: u32) -> Self {
        Self {
            magic: *MAGIC,
            version: VERSION,
            binary_count,
            reserved: [0u8; 48],
        }
    }

    fn as_bytes(&self) -> [u8; 64] {
        let mut bytes = [0u8; 64];
        bytes[0..8].copy_from_slice(&self.magic);
        bytes[8..12].copy_from_slice(&self.version.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.binary_count.to_le_bytes());
        bytes[16..64].copy_from_slice(&self.reserved);
        bytes
    }
}

/// Binary entry in the entry table (sectors 1-63)
///
/// Total size: 64 bytes (8 entries per 512-byte sector)
#[repr(C)]
#[derive(Clone, Copy)]
struct BinaryEntry {
    name: [u8; 32],       // Null-terminated binary name (e.g., "hello_world")
    sector_offset: u64,   // Starting sector (from disk start)
    size_bytes: u64,      // Actual binary size in bytes
    reserved: [u8; 16],   // Padding for future use
}

impl BinaryEntry {
    fn new(name: &str, sector_offset: u64, size_bytes: u64) -> Self {
        let mut name_bytes = [0u8; 32];
        let name_len = name.len().min(31); // Leave room for null terminator
        name_bytes[..name_len].copy_from_slice(&name.as_bytes()[..name_len]);

        Self {
            name: name_bytes,
            sector_offset,
            size_bytes,
            reserved: [0u8; 16],
        }
    }

    fn as_bytes(&self) -> [u8; 64] {
        let mut bytes = [0u8; 64];
        bytes[0..32].copy_from_slice(&self.name);
        bytes[32..40].copy_from_slice(&self.sector_offset.to_le_bytes());
        bytes[40..48].copy_from_slice(&self.size_bytes.to_le_bytes());
        bytes[48..64].copy_from_slice(&self.reserved);
        bytes
    }
}

pub fn create_test_disk() -> Result<()> {
    println!("Creating test disk image...");

    let userspace_dir = Path::new("userspace/tests");
    let userspace_std_dir = Path::new("userspace/tests-std/target/x86_64-breenix/release");
    let output_path = Path::new("target/test_binaries.img");

    // Find all .elf files from userspace/tests/
    let mut binaries = Vec::new();

    if !userspace_dir.exists() {
        bail!("Userspace tests directory not found: {}", userspace_dir.display());
    }

    for entry in fs::read_dir(userspace_dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.extension().and_then(|s| s.to_str()) == Some("elf") {
            if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                let mut file = File::open(&path)?;
                let mut data = Vec::new();
                file.read_to_end(&mut data)?;

                binaries.push((name.to_string(), data));
            }
        }
    }

    // Also include binaries from userspace/tests-std (Rust std tests)
    // These are ELF binaries without the .elf extension, built with cargo
    if userspace_std_dir.exists() {
        // List of known std test binaries to include
        let std_binaries = ["hello_std_real"];

        for bin_name in std_binaries {
            let bin_path = userspace_std_dir.join(bin_name);
            if bin_path.exists() {
                let mut file = File::open(&bin_path)?;
                let mut data = Vec::new();
                file.read_to_end(&mut data)?;

                // Verify it's an ELF file
                if data.len() >= 4 && &data[0..4] == b"\x7fELF" {
                    println!("  Including std test: {} ({} bytes)", bin_name, data.len());
                    binaries.push((bin_name.to_string(), data));
                } else {
                    println!("  Warning: {} is not an ELF file, skipping", bin_name);
                }
            } else {
                println!("  Note: std test binary {} not found (build with: cd userspace/tests-std && cargo build --release)", bin_name);
            }
        }
    }

    if binaries.is_empty() {
        bail!("No .elf files found in {}", userspace_dir.display());
    }

    if binaries.len() > MAX_BINARIES {
        bail!("Too many binaries: {} (max: {})", binaries.len(), MAX_BINARIES);
    }

    // Sort binaries by name for deterministic output
    binaries.sort_by(|a, b| a.0.cmp(&b.0));

    println!("  Found {} test binaries", binaries.len());

    // Create output file
    let mut output = File::create(output_path)?;

    // Write header
    let header = TestDiskHeader::new(binaries.len() as u32);
    let header_bytes = header.as_bytes();
    let mut sector_buffer = vec![0u8; SECTOR_SIZE];
    sector_buffer[..64].copy_from_slice(&header_bytes);
    output.write_all(&sector_buffer)?;

    // Build entry table
    let mut entries = Vec::new();
    let mut current_sector = DATA_START_SECTOR;

    for (name, data) in &binaries {
        let size_bytes = data.len() as u64;
        let entry = BinaryEntry::new(name, current_sector, size_bytes);
        entries.push(entry);

        // Calculate sectors needed for this binary (round up)
        let sectors_needed = (size_bytes + SECTOR_SIZE as u64 - 1) / SECTOR_SIZE as u64;
        current_sector += sectors_needed;
    }

    // Write entry table (sectors 1-63)
    // Each sector holds 8 entries (512 bytes / 64 bytes per entry)
    sector_buffer.fill(0);
    let mut entries_in_current_sector = 0;

    for entry in &entries {
        let entry_bytes = entry.as_bytes();
        let offset = entries_in_current_sector * 64;
        sector_buffer[offset..offset + 64].copy_from_slice(&entry_bytes);
        entries_in_current_sector += 1;

        // Write sector when full (8 entries)
        if entries_in_current_sector == 8 {
            output.write_all(&sector_buffer)?;
            sector_buffer.fill(0);
            entries_in_current_sector = 0;
        }
    }

    // Write partial sector if any entries remain
    if entries_in_current_sector > 0 {
        output.write_all(&sector_buffer)?;
        sector_buffer.fill(0);
    }

    // Pad to sector 128 (entry table in sectors 1-127, data starts at sector 128)
    let entries_sectors_written = (entries.len() + 7) / 8; // Round up
    let padding_sectors = 127_usize.saturating_sub(entries_sectors_written);
    for _ in 0..padding_sectors {
        output.write_all(&sector_buffer)?;
    }

    // Write binary data
    let mut total_bytes = 0u64;
    for (i, (name, data)) in binaries.iter().enumerate() {
        let entry = &entries[i];
        let size_bytes = data.len();
        total_bytes += size_bytes as u64;

        // Calculate sectors needed
        let sectors_needed = (size_bytes + SECTOR_SIZE - 1) / SECTOR_SIZE;
        let start_sector = entry.sector_offset;
        let end_sector = start_sector + sectors_needed as u64 - 1;

        println!("  Added: {} ({} bytes, sectors {}-{})",
                 name, size_bytes, start_sector, end_sector);

        // Write data
        output.write_all(data)?;

        // Pad to sector boundary
        let remainder = size_bytes % SECTOR_SIZE;
        if remainder != 0 {
            let padding = vec![0u8; SECTOR_SIZE - remainder];
            output.write_all(&padding)?;
        }
    }

    let total_sectors = current_sector;
    let total_mb = (total_sectors * SECTOR_SIZE as u64) as f64 / (1024.0 * 1024.0);

    println!("\nTest disk created: {}", output_path.display());
    println!("  Binaries: {}", binaries.len());
    println!("  Data size: {} bytes ({:.2} MB)", total_bytes, total_bytes as f64 / (1024.0 * 1024.0));
    println!("  Disk size: {} sectors ({:.2} MB)", total_sectors, total_mb);

    Ok(())
}
