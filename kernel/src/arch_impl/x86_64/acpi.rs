//! Fixed-table ACPI reader: RSDP -> RSDT/XSDT -> MADT processor entries.
//!
//! Scope, stated so the boundary is not inferred from the name: this module
//! READS four fixed tables and reports what they say. It programs no APIC
//! register, starts no processor, installs no mapping, and interprets no AML.
//! Its single caller is `super::smp::init`.
//!
//! ## Why it needs no mapping of its own
//!
//! `main.rs`'s `BOOTLOADER_CONFIG` sets `config.mappings.physical_memory =
//! Some(Mapping::Dynamic)`, and the pinned bootloader
//! (`kernel/Cargo.toml`'s `rust-osdev/bootloader` rev) maps
//! `[0, max_phys_addr)` at the offset it then reports in
//! `BootInfo::physical_memory_offset`, where `max_phys_addr` is raised to at
//! least 4 GiB precisely so that firmware/MMIO regions below 4 GiB are
//! reachable (`common/src/legacy_memory_region.rs`'s `max_phys_addr()`, whose
//! comment names the local APIC, I/O APIC and PCI BARs as the reason). The
//! kernel's own master PML4 copies the bootloader's PML4 entries into the
//! table it then installs -- including the lower-half loop at
//! `memory/kernel_page_table.rs`'s `build_master_kernel_pml4`, whose comment
//! names "physical memory offset" as one of the mappings it exists to
//! preserve -- which is why the offset window survives the CR3 switch
//! `memory::init` performs before this module runs. On the measured boots of
//! this round that offset was 0x28000000000, a lower-half address, so the
//! lower-half loop is the load-bearing one here.
//!
//! This module therefore relies on exactly one part of that window: physical
//! addresses BELOW `PHYS_READ_CEILING` (4 GiB). A table address at or above
//! the ceiling is refused with `AboveReadCeiling` rather than dereferenced,
//! because establishing that a higher address is mapped would require reading
//! the memory map, which this module deliberately does not do.
//!
//! ## Bounds
//!
//! Each loop below is bounded by a constant, not only by table-supplied
//! lengths: `MAX_TABLE_LENGTH` caps how many bytes a checksum walks,
//! `MAX_ROOT_ENTRIES` caps how many root-table pointers are followed, and
//! `MAX_MADT_ENTRIES` caps how many MADT entries are decoded. A malformed
//! table cannot make any of them run longer than those constants allow: the
//! entry walk also refuses a self-declared entry length below the two-byte
//! ACPI minimum, which is the shape that would otherwise not advance.
//!
//! A table's declared length bounds the reads INTO it as well as the walks
//! OVER it: `census_of_madt` checks that the MADT's declared length reaches
//! the fixed body-header fields at offsets 36..44 before reading them, so a
//! table too short to contain them is refused rather than read past the extent
//! its own checksum covers. The RSDP is the one table here whose length is not
//! reachable that way -- the field lives at offset 20, so the revision byte at
//! 15 is what gates reading it -- and the length it then reports is held to
//! `RSDP_V2_LENGTH` before the extended checksum is walked and the XSDT
//! address at offsets 24..32 is read. `tests/x86_madt_reader_bounds.rs`
//! compiles this module into a host test binary and runs it over synthetic
//! tables that take both of those refusals.
//!
//! There is no allocation here: results travel in a `MadtCensus` value whose
//! fields are fixed-size, and `tests/x86_smp_enum_structure.rs` pins that
//! property and the bounds above at source level.

use core::ptr::read_volatile;

/// How many processor APIC ids one census records.
///
/// This is an ENUMERATION capacity, deliberately not
/// `crate::task::scheduler::MAX_CPUS` (1 on x86 today): what the firmware
/// reports and what the scheduler can dispatch on are different numbers in
/// this PR, and collapsing them would hide the very gap the marker exists to
/// report. Entries beyond this capacity are still COUNTED; only their ids go
/// unrecorded.
pub const MAX_ENUMERATED_CPUS: usize = 64;

/// Physical addresses at or above this are refused rather than read. See the
/// module comment: below 4 GiB is the part of the bootloader's
/// physical-memory window this module can rely on without reading the memory
/// map itself.
const PHYS_READ_CEILING: u64 = 4 * 1024 * 1024 * 1024;

/// Longest ACPI table this reader will checksum or walk. QEMU's RSDT/XSDT and
/// MADT are a few hundred bytes; a length beyond this is treated as a
/// malformed header rather than walked.
const MAX_TABLE_LENGTH: u32 = 64 * 1024;

/// Most root-table (RSDT/XSDT) pointers followed while looking for the MADT.
const MAX_ROOT_ENTRIES: usize = 256;

/// Most MADT interrupt-controller entries decoded in one walk.
const MAX_MADT_ENTRIES: usize = 1024;

/// Bytes of an ACPI System Description Table header.
const SDT_HEADER_LENGTH: u32 = 36;

/// Bytes of an ACPI 2.0+ RSDP: the 20-byte ACPI 1.0 prefix, the 4-byte Length,
/// the 8-byte XSDT address, the extended checksum byte and 3 reserved bytes.
/// A revision >= 2 RSDP whose Length reports less than this does not contain
/// the XSDT address at offsets 24..32, so the reader falls back to the RSDT
/// rather than checksumming and reading past what that length covers.
const RSDP_V2_LENGTH: u32 = 36;

/// Bytes of the MADT's own fixed body header: the 32-bit Local Interrupt
/// Controller Address and the 32-bit Multiple APIC Flags word, which ACPI
/// places between the SDT header and the first interrupt-controller structure.
const MADT_BODY_HEADER_LENGTH: u32 = 8;

/// Shortest MADT this reader will read a body header out of. `sdt_length`
/// enforces only the 36-byte SDT-header minimum, which a MADT declaring a
/// length in `[36, 44)` satisfies while not containing the two fixed fields
/// `census_of_madt` consumes before its first entry. Such a table is refused,
/// because reading those fields would read bytes outside the extent the
/// table's own checksum covers.
const MADT_MIN_LENGTH: u32 = SDT_HEADER_LENGTH + MADT_BODY_HEADER_LENGTH;

/// MADT interrupt-controller structure types this reader decodes.
const MADT_TYPE_LOCAL_APIC: u8 = 0;
const MADT_TYPE_LOCAL_X2APIC: u8 = 9;

/// Declared length of the two structures above, per ACPI 6.5 table 5.22/5.28.
const MADT_LOCAL_APIC_LENGTH: u8 = 8;
const MADT_LOCAL_X2APIC_LENGTH: u8 = 16;

/// Smallest legal MADT entry: the type and length bytes themselves.
const MADT_MIN_ENTRY_LENGTH: u8 = 2;

/// Local APIC / x2APIC flags bit 0: the processor is enabled and usable.
const MADT_FLAG_ENABLED: u32 = 1 << 0;

/// What the firmware's MADT says about the processors on this machine.
///
/// Each field is a plain count or a fixed-size array: this type is `Copy` and
/// carries no allocation.
#[derive(Clone, Copy)]
pub struct MadtCensus {
    /// Processor entries seen: type 0 plus type 9.
    pub processor_entries: u32,
    /// Of those, the ones whose flags carry the Enabled bit.
    pub enabled_entries: u32,
    /// Of those, the ones that were type 9 (Processor Local x2APIC).
    pub x2apic_entries: u32,
    /// Local APIC address the MADT header reports.
    pub local_apic_address: u32,
    /// How many of `apic_ids` are populated.
    pub recorded: usize,
    /// APIC ids in MADT order, for the first `recorded` processor entries.
    pub apic_ids: [u32; MAX_ENUMERATED_CPUS],
}

impl MadtCensus {
    const fn empty() -> Self {
        Self {
            processor_entries: 0,
            enabled_entries: 0,
            x2apic_entries: 0,
            local_apic_address: 0,
            recorded: 0,
            apic_ids: [0; MAX_ENUMERATED_CPUS],
        }
    }

    fn record(&mut self, apic_id: u32, flags: u32, is_x2apic: bool) {
        self.processor_entries = self.processor_entries.saturating_add(1);
        if flags & MADT_FLAG_ENABLED != 0 {
            self.enabled_entries = self.enabled_entries.saturating_add(1);
        }
        if is_x2apic {
            self.x2apic_entries = self.x2apic_entries.saturating_add(1);
        }
        if self.recorded < MAX_ENUMERATED_CPUS {
            self.apic_ids[self.recorded] = apic_id;
            self.recorded += 1;
        }
    }
}

/// Why a census could not be produced. Each variant is reported verbatim in
/// the enumeration marker's `reason=` field, so a refusal names its own cause
/// instead of degrading silently to a CPUID guess.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MadtRefusal {
    /// The bootloader handed the kernel no RSDP address.
    NoRsdpFromBootloader,
    /// A table address was 0, at or above `PHYS_READ_CEILING`, or wrapped.
    AboveReadCeiling,
    /// The RSDP's 8-byte signature is not `RSD PTR `.
    RsdpSignature,
    /// The RSDP's checksum (or, at revision >= 2, its extended checksum) is
    /// nonzero.
    RsdpChecksum,
    /// Neither an RSDT nor an XSDT address usable by this reader.
    NoRootTable,
    /// The root table's signature, length or checksum was refused.
    RootTableHeader,
    /// The root table was walked and carried no `APIC` (MADT) entry.
    NoMadtInRootTable,
    /// The MADT's own signature, length or checksum was refused -- including
    /// a declared length shorter than the fixed body header at offsets 36..44.
    MadtHeader,
    /// A MADT entry declared a length below the two-byte ACPI minimum.
    MadtEntryLength,
}

/// The token this refusal contributes to the marker's `reason=` field.
pub fn refusal_token(refusal: MadtRefusal) -> &'static str {
    match refusal {
        MadtRefusal::NoRsdpFromBootloader => "no_rsdp_from_bootloader",
        MadtRefusal::AboveReadCeiling => "above_read_ceiling",
        MadtRefusal::RsdpSignature => "rsdp_signature",
        MadtRefusal::RsdpChecksum => "rsdp_checksum",
        MadtRefusal::NoRootTable => "no_root_table",
        MadtRefusal::RootTableHeader => "root_table_header",
        MadtRefusal::NoMadtInRootTable => "no_madt_in_root_table",
        MadtRefusal::MadtHeader => "madt_header",
        MadtRefusal::MadtEntryLength => "madt_entry_length",
    }
}

/// A bounded reader over the bootloader's physical-memory window.
#[derive(Clone, Copy)]
struct PhysReader {
    offset: u64,
}

impl PhysReader {
    const fn new(physical_memory_offset: u64) -> Self {
        Self {
            offset: physical_memory_offset,
        }
    }

    /// Whether `[phys, phys + len)` sits inside the window this module relies
    /// on. A physical address of 0 is refused too: ACPI uses 0 as "absent".
    fn readable(&self, phys: u64, len: u64) -> bool {
        if phys == 0 {
            return false;
        }
        match phys.checked_add(len) {
            Some(end) => end <= PHYS_READ_CEILING,
            None => false,
        }
    }

    fn u8_at(&self, phys: u64) -> Option<u8> {
        if !self.readable(phys, 1) {
            return None;
        }
        // SAFETY: `readable` bounded `phys` below PHYS_READ_CEILING, which the
        // module comment establishes is inside the bootloader's
        // physical-memory mapping, and `self.offset` is the offset that
        // mapping was reported at. The read is one byte, volatile because the
        // firmware owns this memory, and the pointer is not written through.
        Some(unsafe { read_volatile((self.offset + phys) as *const u8) })
    }

    /// Little-endian, byte at a time: ACPI table fields are not necessarily
    /// naturally aligned (an XSDT's 8-byte entries sit at header + 8*i,
    /// i.e. 4-byte aligned), and a wide unaligned `read_volatile` is undefined
    /// behaviour.
    fn u32_at(&self, phys: u64) -> Option<u32> {
        let mut value: u32 = 0;
        for index in 0..4u64 {
            value |= u32::from(self.u8_at(phys + index)?) << (8 * index);
        }
        Some(value)
    }

    fn u64_at(&self, phys: u64) -> Option<u64> {
        let mut value: u64 = 0;
        for index in 0..8u64 {
            value |= u64::from(self.u8_at(phys + index)?) << (8 * index);
        }
        Some(value)
    }

    /// True when the bytes `[phys, phys + n)` sum to 0 modulo 256, which is the
    /// ACPI checksum rule for the 4 table kinds this reader touches.
    fn checksum_ok(&self, phys: u64, length: u32) -> Option<bool> {
        if length == 0 || length > MAX_TABLE_LENGTH {
            return None;
        }
        let mut sum: u8 = 0;
        for index in 0..u64::from(length) {
            sum = sum.wrapping_add(self.u8_at(phys + index)?);
        }
        Some(sum == 0)
    }

    fn signature_is(&self, phys: u64, expected: &[u8]) -> Option<bool> {
        for (index, byte) in expected.iter().enumerate() {
            if self.u8_at(phys + index as u64)? != *byte {
                return Some(false);
            }
        }
        Some(true)
    }
}

/// Validate an SDT header at `phys` and return its declared length.
fn sdt_length(reader: &PhysReader, phys: u64, signature: &[u8; 4]) -> Option<u32> {
    if !reader.signature_is(phys, signature)? {
        return None;
    }
    let length = reader.u32_at(phys + 4)?;
    if length < SDT_HEADER_LENGTH || length > MAX_TABLE_LENGTH {
        return None;
    }
    if !reader.checksum_ok(phys, length)? {
        return None;
    }
    Some(length)
}

/// Read the RSDP and return the root table's physical address together with
/// the width of one of its entries (4 for an RSDT, 8 for an XSDT).
fn root_table(reader: &PhysReader, rsdp_phys: u64) -> Result<(u64, u64), MadtRefusal> {
    if !reader.readable(rsdp_phys, 20) {
        return Err(MadtRefusal::AboveReadCeiling);
    }
    let signature_matches = reader
        .signature_is(rsdp_phys, b"RSD PTR ")
        .ok_or(MadtRefusal::AboveReadCeiling)?;
    if !signature_matches {
        return Err(MadtRefusal::RsdpSignature);
    }
    // ACPI 1.0 RSDP: the first 20 bytes sum to 0 modulo 256.
    let first_20_ok = reader
        .checksum_ok(rsdp_phys, 20)
        .ok_or(MadtRefusal::AboveReadCeiling)?;
    if !first_20_ok {
        return Err(MadtRefusal::RsdpChecksum);
    }

    let revision = reader
        .u8_at(rsdp_phys + 15)
        .ok_or(MadtRefusal::AboveReadCeiling)?;
    if revision >= 2 {
        let length = reader
            .u32_at(rsdp_phys + 20)
            .ok_or(MadtRefusal::AboveReadCeiling)?;
        if length >= RSDP_V2_LENGTH && length <= MAX_TABLE_LENGTH {
            let extended_ok = reader
                .checksum_ok(rsdp_phys, length)
                .ok_or(MadtRefusal::AboveReadCeiling)?;
            if !extended_ok {
                return Err(MadtRefusal::RsdpChecksum);
            }
            let xsdt = reader
                .u64_at(rsdp_phys + 24)
                .ok_or(MadtRefusal::AboveReadCeiling)?;
            if reader.readable(xsdt, u64::from(SDT_HEADER_LENGTH)) {
                return Ok((xsdt, 8));
            }
        }
    }

    let rsdt = u64::from(
        reader
            .u32_at(rsdp_phys + 16)
            .ok_or(MadtRefusal::AboveReadCeiling)?,
    );
    if reader.readable(rsdt, u64::from(SDT_HEADER_LENGTH)) {
        return Ok((rsdt, 4));
    }
    Err(MadtRefusal::NoRootTable)
}

/// Walk the MADT body and count its processor entries.
fn census_of_madt(
    reader: &PhysReader,
    madt_phys: u64,
    madt_length: u32,
) -> Result<MadtCensus, MadtRefusal> {
    let mut census = MadtCensus::empty();

    // Before either fixed body-header field is read: `sdt_length` checksummed
    // exactly `madt_length` bytes and vouches for no byte past them, so a
    // table whose declared length does not reach `MADT_MIN_LENGTH` is refused
    // rather than read at offsets it does not cover.
    if madt_length < MADT_MIN_LENGTH {
        return Err(MadtRefusal::MadtHeader);
    }

    census.local_apic_address = reader
        .u32_at(madt_phys + u64::from(SDT_HEADER_LENGTH))
        .ok_or(MadtRefusal::MadtHeader)?;

    // Body starts after the 36-byte header, the 4-byte local APIC address and
    // the 4-byte multiple-APIC flags word -- `MADT_MIN_LENGTH`, which the
    // check above established this table declares at least.
    let mut cursor = u64::from(MADT_MIN_LENGTH);
    let end = u64::from(madt_length);
    let mut decoded = 0usize;
    while cursor + 2 <= end && decoded < MAX_MADT_ENTRIES {
        let entry_type = reader
            .u8_at(madt_phys + cursor)
            .ok_or(MadtRefusal::MadtHeader)?;
        let entry_length = reader
            .u8_at(madt_phys + cursor + 1)
            .ok_or(MadtRefusal::MadtHeader)?;
        if entry_length < MADT_MIN_ENTRY_LENGTH {
            return Err(MadtRefusal::MadtEntryLength);
        }
        if cursor + u64::from(entry_length) > end {
            break;
        }
        match entry_type {
            MADT_TYPE_LOCAL_APIC if entry_length >= MADT_LOCAL_APIC_LENGTH => {
                let apic_id = reader
                    .u8_at(madt_phys + cursor + 3)
                    .ok_or(MadtRefusal::MadtHeader)?;
                let flags = reader
                    .u32_at(madt_phys + cursor + 4)
                    .ok_or(MadtRefusal::MadtHeader)?;
                census.record(u32::from(apic_id), flags, false);
            }
            MADT_TYPE_LOCAL_X2APIC if entry_length >= MADT_LOCAL_X2APIC_LENGTH => {
                let x2apic_id = reader
                    .u32_at(madt_phys + cursor + 4)
                    .ok_or(MadtRefusal::MadtHeader)?;
                let flags = reader
                    .u32_at(madt_phys + cursor + 8)
                    .ok_or(MadtRefusal::MadtHeader)?;
                census.record(x2apic_id, flags, true);
            }
            _ => {}
        }
        cursor += u64::from(entry_length);
        decoded += 1;
    }

    Ok(census)
}

/// Read the firmware's processor enumeration.
///
/// `rsdp_phys` is what the bootloader reported in `BootInfo::rsdp_addr`;
/// `physical_memory_offset` is the offset its physical-memory mapping was
/// reported at. Returns the census on success, or the refusal that stopped
/// the walk.
pub fn read_madt(
    rsdp_phys: Option<u64>,
    physical_memory_offset: u64,
) -> Result<MadtCensus, MadtRefusal> {
    let rsdp_phys = rsdp_phys.ok_or(MadtRefusal::NoRsdpFromBootloader)?;
    let reader = PhysReader::new(physical_memory_offset);

    let (root_phys, entry_width) = root_table(&reader, rsdp_phys)?;
    let root_signature: &[u8; 4] = if entry_width == 8 { b"XSDT" } else { b"RSDT" };
    let root_length =
        sdt_length(&reader, root_phys, root_signature).ok_or(MadtRefusal::RootTableHeader)?;

    let entry_count = ((root_length - SDT_HEADER_LENGTH) as u64 / entry_width) as usize;
    let entry_count = if entry_count > MAX_ROOT_ENTRIES {
        MAX_ROOT_ENTRIES
    } else {
        entry_count
    };

    for index in 0..entry_count {
        let entry_at = root_phys + u64::from(SDT_HEADER_LENGTH) + (index as u64) * entry_width;
        let table_phys = if entry_width == 8 {
            reader.u64_at(entry_at).ok_or(MadtRefusal::RootTableHeader)?
        } else {
            u64::from(reader.u32_at(entry_at).ok_or(MadtRefusal::RootTableHeader)?)
        };
        if !reader.readable(table_phys, u64::from(SDT_HEADER_LENGTH)) {
            continue;
        }
        let is_madt = match reader.signature_is(table_phys, b"APIC") {
            Some(matched) => matched,
            None => continue,
        };
        if !is_madt {
            continue;
        }
        let madt_length =
            sdt_length(&reader, table_phys, b"APIC").ok_or(MadtRefusal::MadtHeader)?;
        return census_of_madt(&reader, table_phys, madt_length);
    }

    Err(MadtRefusal::NoMadtInRootTable)
}
