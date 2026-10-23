//! Fixed-table ACPI reader: RSDP -> RSDT/XSDT -> MADT processor entries, the
//! SPCR's console UART, the MCFG apertures and the FADT's timer and reset
//! register.
//!
//! Scope, stated so the boundary is not inferred from the name: this module
//! READS fixed tables and reports what they say. It programs no APIC
//! register, starts no processor, installs no mapping, and interprets no AML.
//! It supplies processor enumeration and interrupt topology to `smp` and `irq`,
//! the console UART to `serial` (`read_spcr_port`), and the root table's
//! census and the FADT to `platform`.
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
/// `crate::task::scheduler::MAX_CPUS`: what the firmware reports and what the
/// scheduler can dispatch on are different numbers, and collapsing them would
/// hide the very gap the marker exists to report. Entries beyond this capacity
/// are still COUNTED; only their ids go unrecorded.
pub const MAX_ENUMERATED_CPUS: usize = 64;
pub const MAX_IO_APICS: usize = 16;

#[derive(Clone, Copy)]
pub struct IoApic {
    pub address: u32,
    pub gsi_base: u32,
}

#[derive(Clone, Copy)]
pub struct SourceOverride {
    pub source: u8,
    pub gsi: u32,
    pub flags: u16,
}

/// How many Local APIC NMI structures (MADT types 4 and 10) one census keeps.
pub const MAX_LOCAL_NMIS: usize = MAX_ENUMERATED_CPUS;

/// The processor UID a Local APIC NMI structure uses to name each processor at
/// once: 0xFF in a type-4 entry and 0xFFFFFFFF in a type-10 entry, both
/// recorded as this value.
pub const ALL_PROCESSORS: u32 = u32::MAX;

/// One Local APIC NMI structure: which processor's local interrupt pin
/// (`lint`, 0 or 1) the platform wires to NMI, with the MPS INTI polarity and
/// trigger flags (ACPI 6.5, 5.2.12.7 and 5.2.12.13).
#[derive(Clone, Copy)]
pub struct LocalNmi {
    pub uid: u32,
    pub flags: u16,
    pub lint: u8,
}

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
const MADT_TYPE_LOCAL_APIC_NMI: u8 = 4;
const MADT_TYPE_LOCAL_X2APIC: u8 = 9;
const MADT_TYPE_LOCAL_X2APIC_NMI: u8 = 10;

/// Declared length of the two structures above, per ACPI 6.5 table 5.22/5.28.
const MADT_LOCAL_APIC_LENGTH: u8 = 8;
const MADT_LOCAL_X2APIC_LENGTH: u8 = 16;
const MADT_LOCAL_APIC_NMI_LENGTH: u8 = 6;
const MADT_LOCAL_X2APIC_NMI_LENGTH: u8 = 12;

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
    pub local_apic_override: Option<u64>,
    pub io_apics: [IoApic; MAX_IO_APICS],
    pub io_apic_count: usize,
    pub interrupt_topology_supported: bool,
    pub qemu_firmware: bool,
    pub overrides: [SourceOverride; 16],
    pub override_count: usize,
    /// Local APIC NMI structures, in MADT order; entries past
    /// `MAX_LOCAL_NMIS`, and ones naming a pin other than LINT0 or LINT1, are
    /// not kept.
    pub local_nmis: [LocalNmi; MAX_LOCAL_NMIS],
    pub local_nmi_count: usize,
    /// How many of `apic_ids` are populated.
    pub recorded: usize,
    /// APIC ids in MADT order, for the first `recorded` processor entries.
    pub apic_ids: [u32; MAX_ENUMERATED_CPUS],
    /// ACPI processor UIDs of those entries, which Local APIC NMI structures
    /// name processors by.
    pub apic_uids: [u32; MAX_ENUMERATED_CPUS],
    /// Whether each of those entries carries the Enabled flag.
    pub apic_enabled: [bool; MAX_ENUMERATED_CPUS],
}

impl MadtCensus {
    const fn empty() -> Self {
        Self {
            processor_entries: 0,
            enabled_entries: 0,
            x2apic_entries: 0,
            local_apic_address: 0,
            local_apic_override: None,
            io_apics: [IoApic {
                address: 0,
                gsi_base: 0,
            }; MAX_IO_APICS],
            io_apic_count: 0,
            interrupt_topology_supported: true,
            qemu_firmware: false,
            overrides: [SourceOverride {
                source: 0,
                gsi: 0,
                flags: 0,
            }; 16],
            override_count: 0,
            local_nmis: [LocalNmi {
                uid: 0,
                flags: 0,
                lint: 0,
            }; MAX_LOCAL_NMIS],
            local_nmi_count: 0,
            recorded: 0,
            apic_ids: [0; MAX_ENUMERATED_CPUS],
            apic_uids: [0; MAX_ENUMERATED_CPUS],
            apic_enabled: [false; MAX_ENUMERATED_CPUS],
        }
    }

    /// The Local APIC NMI structure that applies to the processor with local
    /// APIC id `apic_id`: the first one naming its UID or `ALL_PROCESSORS`. A
    /// processor this census did not record matches only an `ALL_PROCESSORS`
    /// entry.
    pub fn local_nmi_for(&self, apic_id: u32) -> Option<LocalNmi> {
        let uid = (0..self.recorded)
            .find(|&index| self.apic_ids[index] == apic_id)
            .map(|index| self.apic_uids[index]);
        self.local_nmis[..self.local_nmi_count]
            .iter()
            .copied()
            .find(|entry| entry.uid == ALL_PROCESSORS || Some(entry.uid) == uid)
    }

    fn record_local_nmi(&mut self, uid: u32, flags: u16, lint: u8) {
        if lint > 1 || self.local_nmi_count == MAX_LOCAL_NMIS {
            return;
        }
        self.local_nmis[self.local_nmi_count] = LocalNmi { uid, flags, lint };
        self.local_nmi_count += 1;
    }

    fn record(&mut self, apic_id: u32, uid: u32, flags: u32, is_x2apic: bool) {
        self.processor_entries = self.processor_entries.saturating_add(1);
        if flags & MADT_FLAG_ENABLED != 0 {
            self.enabled_entries = self.enabled_entries.saturating_add(1);
        }
        if is_x2apic {
            self.x2apic_entries = self.x2apic_entries.saturating_add(1);
        }
        if self.recorded < MAX_ENUMERATED_CPUS {
            self.apic_ids[self.recorded] = apic_id;
            self.apic_uids[self.recorded] = uid;
            self.apic_enabled[self.recorded] = flags & MADT_FLAG_ENABLED != 0;
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

    census.qemu_firmware = reader
        .signature_is(madt_phys + 10, b"BOCHS ")
        .unwrap_or(false);
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
                let uid = reader
                    .u8_at(madt_phys + cursor + 2)
                    .ok_or(MadtRefusal::MadtHeader)?;
                let apic_id = reader
                    .u8_at(madt_phys + cursor + 3)
                    .ok_or(MadtRefusal::MadtHeader)?;
                let flags = reader
                    .u32_at(madt_phys + cursor + 4)
                    .ok_or(MadtRefusal::MadtHeader)?;
                census.record(u32::from(apic_id), u32::from(uid), flags, false);
            }
            MADT_TYPE_LOCAL_X2APIC if entry_length >= MADT_LOCAL_X2APIC_LENGTH => {
                let x2apic_id = reader
                    .u32_at(madt_phys + cursor + 4)
                    .ok_or(MadtRefusal::MadtHeader)?;
                let flags = reader
                    .u32_at(madt_phys + cursor + 8)
                    .ok_or(MadtRefusal::MadtHeader)?;
                let uid = reader
                    .u32_at(madt_phys + cursor + 12)
                    .ok_or(MadtRefusal::MadtHeader)?;
                census.record(x2apic_id, uid, flags, true);
            }
            MADT_TYPE_LOCAL_APIC_NMI if entry_length >= MADT_LOCAL_APIC_NMI_LENGTH => {
                let uid = reader
                    .u8_at(madt_phys + cursor + 2)
                    .ok_or(MadtRefusal::MadtHeader)?;
                let flags = u16::from_le_bytes([
                    reader.u8_at(madt_phys + cursor + 3).ok_or(MadtRefusal::MadtHeader)?,
                    reader.u8_at(madt_phys + cursor + 4).ok_or(MadtRefusal::MadtHeader)?,
                ]);
                let lint = reader
                    .u8_at(madt_phys + cursor + 5)
                    .ok_or(MadtRefusal::MadtHeader)?;
                let uid = if uid == 0xff { ALL_PROCESSORS } else { u32::from(uid) };
                census.record_local_nmi(uid, flags, lint);
            }
            MADT_TYPE_LOCAL_X2APIC_NMI if entry_length >= MADT_LOCAL_X2APIC_NMI_LENGTH => {
                let flags = u16::from_le_bytes([
                    reader.u8_at(madt_phys + cursor + 2).ok_or(MadtRefusal::MadtHeader)?,
                    reader.u8_at(madt_phys + cursor + 3).ok_or(MadtRefusal::MadtHeader)?,
                ]);
                let uid = reader
                    .u32_at(madt_phys + cursor + 4)
                    .ok_or(MadtRefusal::MadtHeader)?;
                let lint = reader
                    .u8_at(madt_phys + cursor + 8)
                    .ok_or(MadtRefusal::MadtHeader)?;
                census.record_local_nmi(uid, flags, lint);
            }
            1 if entry_length >= 12 => {
                if census.io_apic_count == MAX_IO_APICS {
                    census.interrupt_topology_supported = false;
                    cursor += u64::from(entry_length);
                    decoded += 1;
                    continue;
                }
                census.io_apics[census.io_apic_count] = IoApic {
                    address: reader
                        .u32_at(madt_phys + cursor + 4)
                        .ok_or(MadtRefusal::MadtHeader)?,
                    gsi_base: reader
                        .u32_at(madt_phys + cursor + 8)
                        .ok_or(MadtRefusal::MadtHeader)?,
                };
                census.io_apic_count += 1;
            }
            2 if entry_length >= 10 => {
                let bus = reader
                    .u8_at(madt_phys + cursor + 2)
                    .ok_or(MadtRefusal::MadtHeader)?;
                let source = reader
                    .u8_at(madt_phys + cursor + 3)
                    .ok_or(MadtRefusal::MadtHeader)?;
                if bus != 0 || source >= 16 || census.override_count == 16 {
                    census.interrupt_topology_supported = false;
                    cursor += u64::from(entry_length);
                    decoded += 1;
                    continue;
                }
                let flags = u16::from(
                    reader
                        .u8_at(madt_phys + cursor + 8)
                        .ok_or(MadtRefusal::MadtHeader)?,
                ) | (u16::from(
                    reader
                        .u8_at(madt_phys + cursor + 9)
                        .ok_or(MadtRefusal::MadtHeader)?,
                ) << 8);
                if flags & !15 != 0 || flags & 3 == 2 || (flags >> 2) & 3 == 2 {
                    census.interrupt_topology_supported = false;
                    cursor += u64::from(entry_length);
                    decoded += 1;
                    continue;
                }
                census.overrides[census.override_count] = SourceOverride {
                    source,
                    gsi: reader
                        .u32_at(madt_phys + cursor + 4)
                        .ok_or(MadtRefusal::MadtHeader)?,
                    flags,
                };
                census.override_count += 1;
            }
            5 if entry_length >= 12 => {
                census.local_apic_override = Some(
                    reader
                        .u64_at(madt_phys + cursor + 4)
                        .ok_or(MadtRefusal::MadtHeader)?,
                );
            }
            _ => {}
        }
        cursor += u64::from(entry_length);
        decoded += 1;
    }

    Ok(census)
}

/// Bytes of an SPCR through its base address: the SDT header, the interface
/// type, three reserved bytes and the 12-byte Generic Address Structure, per
/// Microsoft's published Serial Port Console Redirection Table specification.
const SPCR_MIN_LENGTH: u32 = 52;

/// SPCR interface types a 16550 driver serves: a full 16550, a full 16450, and
/// a 16550-compatible UART whose registers the base address describes.
const SPCR_16550_INTERFACES: [u8; 3] = [0x00, 0x01, 0x12];

/// The Generic Address Structure's address space id for system I/O ports.
const GAS_SYSTEM_IO: u8 = 1;

/// The I/O port of the console UART the firmware's SPCR names, when it names a
/// 16550-class UART in I/O space. An absent SPCR, a UART of another kind or in
/// memory space, and a refused table give no port
/// (`tests/x86_madt_reader_bounds.rs`).
pub fn read_spcr_port(rsdp_phys: Option<u64>, physical_memory_offset: u64) -> Option<u16> {
    let reader = PhysReader::new(physical_memory_offset);
    let (spcr, length) = find_table(&reader, rsdp_phys?, b"SPCR")?;
    if length < SPCR_MIN_LENGTH {
        return None;
    }
    let interface = reader.u8_at(spcr + 36)?;
    if !SPCR_16550_INTERFACES.contains(&interface) || reader.u8_at(spcr + 40)? != GAS_SYSTEM_IO {
        return None;
    }
    let address = reader.u64_at(spcr + 44)?;
    u16::try_from(address).ok().filter(|&port| port != 0)
}

/// One firmware-described PCI configuration aperture (PCI Firmware
/// Specification, MCFG allocation structure). Reading this is inventory only;
/// it does not map ECAM or replace the PCI configuration mechanism.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McfgAllocation {
    pub base: u64,
    pub segment: u16,
    pub first_bus: u8,
    pub last_bus: u8,
}

/// Visit the allocations in a checksum-validated MCFG, refusing malformed
/// lengths, reversed bus ranges, unaligned bases and overflowing apertures.
pub fn read_mcfg(
    rsdp_phys: Option<u64>,
    physical_memory_offset: u64,
    mut visit: impl FnMut(McfgAllocation),
) -> Option<usize> {
    let reader = PhysReader::new(physical_memory_offset);
    let (table, length) = find_table(&reader, rsdp_phys?, b"MCFG")?;
    if length < 44 || (length - 44) % 16 != 0 {
        return None;
    }
    let count = (length - 44) / 16;
    // Validate the table before publishing allocations.
    let allocation = |index: u32| -> Option<McfgAllocation> {
        let at = table + 44 + u64::from(index) * 16;
        let base = reader.u64_at(at)?;
        let segment = u16::from_le_bytes([reader.u8_at(at + 8)?, reader.u8_at(at + 9)?]);
        let first_bus = reader.u8_at(at + 10)?;
        let last_bus = reader.u8_at(at + 11)?;
        if base == 0 || base & 0xfffff != 0 || first_bus > last_bus {
            return None;
        }
        base.checked_add((u64::from(last_bus) + 1) << 20)?;
        Some(McfgAllocation { base, segment, first_bus, last_bus })
    };
    for index in 0..count { allocation(index)?; }
    for index in 0..count { visit(allocation(index)?); }
    Some(count as usize)
}

/// IPMI 2.0 Appendix C3: a fixed, byte-access KCS interface in system I/O.
pub fn read_kcs_ports(rsdp: Option<u64>, offset: u64) -> Option<(u16, u16)> {
    if let Some(ports) = smbios_kcs(rsdp, offset) { return Some(ports); }
    let reader = PhysReader::new(offset);
    let (table, length) = find_table(&reader, rsdp?, b"SPMI")?;
    if length < 64 || reader.u8_at(table + 36)? != 1 || reader.u8_at(table + 43)? & 1 != 0 {
        return None;
    }
    let gas = generic_address(&reader, table + 48)?;
    if gas.space != 1 || gas.bit_offset != 0 || !matches!(gas.bit_width, 8 | 16) || gas.access_size > 1 {
        return None;
    }
    let port = u16::try_from(gas.address).ok()?;
    if port == 0 || port == u16::MAX { return None; }
    Some((port, port + 1))
}

/// The first root-table entry carrying `signature` whose header and checksum
/// are sound, with its declared length. The walk is capped by
/// `MAX_ROOT_ENTRIES`, as `read_madt`'s is.
fn find_table(reader: &PhysReader, rsdp_phys: u64, signature: &[u8; 4]) -> Option<(u64, u32)> {
    find_table_where(reader, rsdp_phys, signature, |_, _| true)
}

fn find_table_where(reader: &PhysReader, rsdp_phys: u64, signature: &[u8; 4], accept: impl Fn(u64, u32) -> bool) -> Option<(u64, u32)> {
    let (root_phys, entry_width) = root_table(reader, rsdp_phys).ok()?;
    let root_signature: &[u8; 4] = if entry_width == 8 { b"XSDT" } else { b"RSDT" };
    let root_length = sdt_length(reader, root_phys, root_signature)?;
    let entry_count = ((root_length - SDT_HEADER_LENGTH) as u64 / entry_width) as usize;
    for index in 0..entry_count.min(MAX_ROOT_ENTRIES) {
        let entry_at = root_phys + u64::from(SDT_HEADER_LENGTH) + (index as u64) * entry_width;
        let table_phys = if entry_width == 8 {
            reader.u64_at(entry_at)?
        } else {
            u64::from(reader.u32_at(entry_at)?)
        };
        if !reader.readable(table_phys, u64::from(SDT_HEADER_LENGTH)) {
            continue;
        }
        if reader.signature_is(table_phys, signature) == Some(true) {
            if let Some(length) = sdt_length(reader, table_phys, signature) {
                if accept(table_phys, length) { return Some((table_phys, length)); }
            }
        }
    }
    None
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
            reader
                .u64_at(entry_at)
                .ok_or(MadtRefusal::RootTableHeader)?
        } else {
            u64::from(
                reader
                    .u32_at(entry_at)
                    .ok_or(MadtRefusal::RootTableHeader)?,
            )
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

/// Visit the signature of each table the root table points at, in root-table
/// order, after the root table's own (`RSDT` or `XSDT`). A pointer this reader
/// cannot read is skipped. Returns how many signatures were visited; no value
/// when the RSDP or root table is refused. The walk is capped by
/// `MAX_ROOT_ENTRIES`, as `read_madt`'s is.
pub fn visit_table_signatures(
    rsdp_phys: Option<u64>,
    physical_memory_offset: u64,
    mut visit: impl FnMut([u8; 4]),
) -> Option<usize> {
    let reader = PhysReader::new(physical_memory_offset);
    let (root_phys, entry_width) = root_table(&reader, rsdp_phys?).ok()?;
    let root_signature: &[u8; 4] = if entry_width == 8 { b"XSDT" } else { b"RSDT" };
    let root_length = sdt_length(&reader, root_phys, root_signature)?;
    visit(*root_signature);
    let mut visited = 1;
    let entry_count = ((root_length - SDT_HEADER_LENGTH) as u64 / entry_width) as usize;
    for index in 0..entry_count.min(MAX_ROOT_ENTRIES) {
        let entry_at = root_phys + u64::from(SDT_HEADER_LENGTH) + (index as u64) * entry_width;
        let table_phys = if entry_width == 8 {
            reader.u64_at(entry_at)?
        } else {
            u64::from(reader.u32_at(entry_at)?)
        };
        if !reader.readable(table_phys, u64::from(SDT_HEADER_LENGTH)) {
            continue;
        }
        let mut signature = [0u8; 4];
        for (offset, byte) in signature.iter_mut().enumerate() {
            *byte = reader.u8_at(table_phys + offset as u64)?;
        }
        visit(signature);
        visited += 1;
    }
    Some(visited)
}

/// An ACPI Generic Address Structure (ACPI 6.5, 5.2.3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenericAddress {
    /// 0 system memory, 1 system I/O, others as the specification lists.
    pub space: u8,
    pub bit_width: u8,
    pub bit_offset: u8,
    pub access_size: u8,
    pub address: u64,
}

/// What this kernel reads from the Fixed ACPI Description Table (ACPI 6.5,
/// 5.2.9): the power-management timer, the reset register, the SCI and the
/// IA-PC boot architecture flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fadt {
    pub revision: u8,
    pub sci_interrupt: u16,
    /// The PM timer block: the 64-bit `X_PM_TMR_BLK` when the table has one
    /// with a nonzero address, else the 32-bit `PM_TMR_BLK` as a system I/O
    /// port when `PM_TMR_LEN` is 4; empty when neither is present.
    pub pm_timer: Option<GenericAddress>,
    /// `TMR_VAL_EXT`: the timer counts 32 bits rather than 24.
    pub pm_timer_32bit: bool,
    /// `RESET_REG` and `RESET_VALUE`, when `RESET_REG_SUP` is set and the
    /// table is long enough to hold them.
    pub reset: Option<(GenericAddress, u8)>,
    pub boot_arch: u16,
    pub flags: u32,
}

impl Fadt {
    /// IA-PC boot architecture flag 3: the OS must not enable MSI on this
    /// platform.
    pub fn msi_not_supported(&self) -> bool {
        self.boot_arch & (1 << 3) != 0
    }

    /// Flag 20: the platform has no fixed ACPI hardware, the PM timer included.
    pub fn hardware_reduced(&self) -> bool {
        self.flags & (1 << 20) != 0
    }
}

/// FADT field offsets, from ACPI 6.5 table 5.9.
const FADT_SCI_INT: u64 = 46;
const FADT_PM_TMR_BLK: u64 = 76;
const FADT_PM_TMR_LEN: u64 = 91;
const FADT_IAPC_BOOT_ARCH: u64 = 109;
const FADT_FLAGS: u64 = 112;
const FADT_RESET_REG: u64 = 116;
const FADT_RESET_VALUE: u64 = 128;
const FADT_X_PM_TMR_BLK: u64 = 208;
/// The ACPI 1.0 table ends after the flags word; the reset register and value
/// need 129 bytes, the extended timer block 220.
const FADT_MIN_LENGTH: u32 = 116;
const FADT_TMR_VAL_EXT: u32 = 1 << 8;
const FADT_RESET_REG_SUP: u32 = 1 << 10;

fn generic_address(reader: &PhysReader, at: u64) -> Option<GenericAddress> {
    Some(GenericAddress {
        space: reader.u8_at(at)?,
        bit_width: reader.u8_at(at + 1)?,
        bit_offset: reader.u8_at(at + 2)?,
        access_size: reader.u8_at(at + 3)?,
        address: reader.u64_at(at + 4)?,
    })
}

/// Read the checksum-validated FADT (signature `FACP`); no value when the
/// table is absent, refused or shorter than the ACPI 1.0 layout.
pub fn read_fadt(rsdp_phys: Option<u64>, physical_memory_offset: u64) -> Option<Fadt> {
    let reader = PhysReader::new(physical_memory_offset);
    let (table, length) = find_table(&reader, rsdp_phys?, b"FACP")?;
    if length < FADT_MIN_LENGTH {
        return None;
    }
    let length = u64::from(length);
    let flags = reader.u32_at(table + FADT_FLAGS)?;
    let extended_timer = if length >= FADT_X_PM_TMR_BLK + 12 {
        Some(generic_address(&reader, table + FADT_X_PM_TMR_BLK)?).filter(|gas| gas.address != 0)
    } else {
        None
    };
    let legacy_timer = reader.u32_at(table + FADT_PM_TMR_BLK)?;
    let pm_timer = extended_timer.or_else(|| {
        (legacy_timer != 0 && reader.u8_at(table + FADT_PM_TMR_LEN) == Some(4)).then_some(GenericAddress {
            space: 1,
            bit_width: 32,
            bit_offset: 0,
            access_size: 0,
            address: u64::from(legacy_timer),
        })
    });
    let reset = if flags & FADT_RESET_REG_SUP != 0 && length > FADT_RESET_VALUE {
        let register = generic_address(&reader, table + FADT_RESET_REG)?;
        let value = reader.u8_at(table + FADT_RESET_VALUE)?;
        (register.address != 0).then_some((register, value))
    } else {
        None
    };
    Some(Fadt {
        revision: reader.u8_at(table + 8)?,
        sci_interrupt: u16::from_le_bytes([
            reader.u8_at(table + FADT_SCI_INT)?,
            reader.u8_at(table + FADT_SCI_INT + 1)?,
        ]),
        pm_timer,
        pm_timer_32bit: flags & FADT_TMR_VAL_EXT != 0,
        reset,
        boot_arch: u16::from_le_bytes([
            reader.u8_at(table + FADT_IAPC_BOOT_ARCH)?,
            reader.u8_at(table + FADT_IAPC_BOOT_ARCH + 1)?,
        ]),
        flags,
    })
}

// The delivery stub and installed manager preserve UEFI's SMBIOS pointer in a
// GUID-named ACPI UEFI data table. Parse SMBIOS 2/3 and Type 38 from DMTF DSP0134
// sections 5.2 and 7.39 / IPMI 2.0 Appendix C1, without BIOS address guesses.
fn smbios_kcs(rsdp: Option<u64>, offset: u64) -> Option<(u16, u16)> {
    const ID: [u8; 16] = [0x0b,0x76,0xb1,0x32,0x04,0x68,0x9c,0x4f,0xab,0x21,0x67,0xf5,0x8f,0x56,0x42,0xe1];
    let r = PhysReader::new(offset);
    let (handoff, length) = find_table_where(&r, rsdp?, b"UEFI", |at, len| {
        len >= 62 && (0..16).all(|i| r.u8_at(at + 36 + i) == Some(ID[i as usize]))
    })?;
    let data = u16::from_le_bytes([r.u8_at(handoff + 52)?, r.u8_at(handoff + 53)?]) as u32;
    if data < 54 || data.checked_add(8)? > length { return None; }
    let entry = r.u64_at(handoff + u64::from(data))?;
    let (table, size) = if r.signature_is(entry, b"_SM_")? {
        let len = r.u8_at(entry + 5)?;
        if len < 31 || len > 64 || !r.signature_is(entry + 16, b"_DMI")? || r.u8_at(entry + 20)? != b'_' || !r.checksum_ok(entry, u32::from(len))? || !r.checksum_ok(entry + 16, 15)? { return None; }
        let size = u16::from_le_bytes([r.u8_at(entry + 22)?, r.u8_at(entry + 23)?]);
        (u64::from(r.u32_at(entry + 24)?), u32::from(size))
    } else {
        if !r.signature_is(entry, b"_SM3")? || r.u8_at(entry + 4)? != b'_' { return None; }
        let len = r.u8_at(entry + 6)?;
        if len < 24 || len > 64 || !r.checksum_ok(entry, u32::from(len))? { return None; }
        (r.u64_at(entry + 16)?, r.u32_at(entry + 12)?)
    };
    if size < 4 || size > 1024 * 1024 || !r.readable(table, u64::from(size)) { return None; }
    let end = table + u64::from(size);
    let mut at = table;
    for _ in 0..4096 {
        if at + 4 > end { return None; }
        let kind = r.u8_at(at)?;
        let len = u64::from(r.u8_at(at + 1)?);
        if len < 4 || at + len > end || kind == 127 { return None; }
        if kind == 38 {
            if len < 18 || r.u8_at(at + 4)? != 1 { return None; }
            let base = r.u64_at(at + 8)?;
            let flags = r.u8_at(at + 16)?;
            if base & 1 == 0 { return None; } // memory-mapped KCS is not supported here
            let data = u16::try_from((base & !1) | u64::from((flags >> 4) & 1)).ok()?;
            let stride = match flags >> 6 { 0 => 1, 1 => 4, 2 => 16, _ => return None };
            if data == 0 { return None; }
            return Some((data, data.checked_add(stride)?));
        }
        at += len;
        // String sets end with a double NUL. Keep the walk within the entry point's extent.
        while at + 1 < end && (r.u8_at(at)? != 0 || r.u8_at(at + 1)? != 0) { at += 1; }
        if at + 1 >= end { return None; }
        at += 2;
    }
    None
}
