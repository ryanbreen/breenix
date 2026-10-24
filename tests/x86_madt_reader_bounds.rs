//! Behavioural bounds tests for the x86_64 MADT reader (#814 PR-1, #629).
//!
//! `tests/x86_smp_enum_structure.rs` reads this reader's SOURCE. This file RUNS
//! it: the module below is `kernel/src/arch_impl/x86_64/acpi.rs` itself,
//! compiled into this test binary. That is possible because the reader's only
//! import is `core::ptr::read_volatile`, so the host executes the same source
//! the kernel does, over synthetic ACPI tables this file builds byte by byte.
//!
//! How the synthetic firmware is reached: the reader turns a physical address
//! into a pointer by adding the `physical_memory_offset` it is handed, so an
//! image placed at a chosen fake physical base is addressed by handing it
//! `image_pointer - IMAGE_BASE`. The 5 fake table addresses used here (an
//! RSDP, an RSDT, an XSDT and 2 MADTs) sit far below the reader's own 4 GiB
//! read ceiling, so what each test measures is the length check it is about
//! rather than a ceiling refusal.
//!
//! What these tests reach: what the reader decides about a table it is handed
//! -- which tables it refuses, and what it counts when it accepts one. What
//! they do NOT reach: that the kernel hands it the RSDP the firmware really
//! reported, and that the physical-memory window it reads through is mapped at
//! boot. Those are boot properties, and `docker/qemu/run-x86-smp-enum-gate.sh`
//! is their evidence.

#[path = "../kernel/src/arch_impl/x86_64/acpi.rs"]
mod acpi;

use acpi::{read_madt, read_spcr_port, refusal_token, MadtCensus, MadtRefusal};

/// The fake physical address the synthetic image's first byte stands at. Well
/// below the reader's `PHYS_READ_CEILING`, so a refusal in this file is a
/// length decision rather than a ceiling one.
const IMAGE_BASE: u64 = 0x0002_0000;

/// Bytes of synthetic physical memory: room for the 6 tables laid out below.
const IMAGE_LENGTH: usize = 4096;

/// Where each synthetic table sits, as a fake physical address.
const RSDP: u64 = IMAGE_BASE + 0x0000;
const RSDT: u64 = IMAGE_BASE + 0x0100;
const XSDT: u64 = IMAGE_BASE + 0x0180;
const MADT: u64 = IMAGE_BASE + 0x0200;
const MADT_BEHIND_THE_XSDT: u64 = IMAGE_BASE + 0x0300;

/// A local APIC address no real machine would report, so a test can tell a
/// value the reader took from the table apart from a zeroed field.
const SENTINEL_LOCAL_APIC_ADDRESS: u32 = 0xDEAD_BEEF;

/// The flags bit ACPI gives "this processor is enabled".
const ENABLED: u32 = 1;

/// A block of synthetic physical memory, addressed by fake physical address.
struct Image {
    bytes: Vec<u8>,
}

impl Image {
    fn new() -> Self {
        Self {
            bytes: vec![0u8; IMAGE_LENGTH],
        }
    }

    fn index(&self, phys: u64) -> usize {
        let offset = phys
            .checked_sub(IMAGE_BASE)
            .expect("a test address below the image base");
        assert!(
            (offset as usize) < IMAGE_LENGTH,
            "a test address past the end of the image: {phys:#x}"
        );
        offset as usize
    }

    fn put(&mut self, phys: u64, data: &[u8]) {
        let at = self.index(phys);
        self.bytes[at..at + data.len()].copy_from_slice(data);
    }

    fn put_u32(&mut self, phys: u64, value: u32) {
        self.put(phys, &value.to_le_bytes());
    }

    fn put_u64(&mut self, phys: u64, value: u64) {
        self.put(phys, &value.to_le_bytes());
    }

    fn byte_at(&self, phys: u64) -> u8 {
        self.bytes[self.index(phys)]
    }

    /// Set the checksum byte at `field` so that `length` bytes from `table` sum
    /// to 0 modulo 256, which is the checksum rule the reader applies.
    fn seal(&mut self, table: u64, length: u32, field: u64) {
        assert!(
            field >= table && field < table + u64::from(length),
            "a checksum field outside the span it seals"
        );
        let field_index = self.index(field);
        self.bytes[field_index] = 0;
        let start = self.index(table);
        let mut sum: u8 = 0;
        for byte in &self.bytes[start..start + length as usize] {
            sum = sum.wrapping_add(*byte);
        }
        self.bytes[field_index] = 0u8.wrapping_sub(sum);
    }

    /// The `physical_memory_offset` the reader must be handed for its physical
    /// addresses to land inside this image.
    fn physical_memory_offset(&self) -> u64 {
        self.bytes.as_ptr() as u64 - IMAGE_BASE
    }
}

/// The 36-byte header an ACPI system description table starts with. The
/// checksum byte is left 0; the caller seals the table once its body is
/// written.
fn put_sdt_header(image: &mut Image, table: u64, signature: &[u8; 4], declared_length: u32) {
    image.put(table, signature);
    image.put_u32(table + 4, declared_length);
    image.put(table + 8, &[1u8]); // revision
    image.put(table + 9, &[0u8]); // checksum
    image.put(table + 10, b"BREENX");
    image.put(table + 16, b"BREENIXT");
    image.put_u32(table + 24, 1);
    image.put(table + 28, b"BRNX");
    image.put_u32(table + 32, 1);
}

/// An ACPI 1.0 RSDP pointing at one RSDT.
fn put_rsdp_v1(image: &mut Image, rsdt: u64) {
    image.put(RSDP, b"RSD PTR ");
    image.put(RSDP + 8, &[0u8]); // checksum, sealed below
    image.put(RSDP + 9, b"BREENX");
    image.put(RSDP + 15, &[0u8]); // revision 0: ACPI 1.0, RSDT only
    image.put_u32(RSDP + 16, rsdt as u32);
    image.seal(RSDP, 20, RSDP + 8);
}

/// A root table listing `tables`: an RSDT, whose entries are 32-bit, or an
/// XSDT, whose entries are 64-bit.
fn put_root_table(
    image: &mut Image,
    root: u64,
    signature: &[u8; 4],
    entry_width: u64,
    tables: &[u64],
) {
    let declared_length = 36 + entry_width as u32 * tables.len() as u32;
    put_sdt_header(image, root, signature, declared_length);
    for (index, table) in tables.iter().enumerate() {
        let at = root + 36 + entry_width * index as u64;
        if entry_width == 8 {
            image.put_u64(at, *table);
        } else {
            image.put_u32(at, *table as u32);
        }
    }
    image.seal(root, declared_length, root + 9);
}

fn put_rsdt(image: &mut Image, root: u64, tables: &[u64]) {
    put_root_table(image, root, b"RSDT", 4, tables);
}

fn put_xsdt(image: &mut Image, root: u64, tables: &[u64]) {
    put_root_table(image, root, b"XSDT", 8, tables);
}

/// An ACPI 2.0+ RSDP whose Length field DECLARES `declared_length`, carrying
/// both a 32-bit RSDT address and a 64-bit XSDT address. Both checksums are
/// sealed -- the 20-byte one over the ACPI 1.0 prefix, the extended one over
/// exactly the declared bytes -- so a reader that trusts the declared length
/// finds it valid, and what the tests below measure is which root table the
/// reader follows, not a checksum refusal.
fn put_rsdp_v2(image: &mut Image, rsdt: u64, xsdt: u64, declared_length: u32) {
    image.put(RSDP, b"RSD PTR ");
    image.put(RSDP + 8, &[0u8]); // checksum, sealed below
    image.put(RSDP + 9, b"BREENX");
    image.put(RSDP + 15, &[2u8]); // revision 2: an XSDT may be present
    image.put_u32(RSDP + 16, rsdt as u32);
    image.put_u32(RSDP + 20, declared_length);
    image.put_u64(RSDP + 24, xsdt);
    image.put(RSDP + 32, &[0u8]); // extended checksum, sealed below
    image.seal(RSDP, 20, RSDP + 8);
    image.seal(RSDP, declared_length, RSDP + 32);
}

/// One MADT behind the RSDT and a DIFFERENT one behind the XSDT, so which root
/// table the reader followed is readable off the census it returns: one
/// processor means the RSDT, four means the XSDT.
fn two_root_tables(declared_rsdp_length: u32) -> Image {
    let mut image = Image::new();

    let one = local_apic_entry(0, 0, ENABLED);
    put_madt(&mut image, MADT, 0xFEE0_0000, &one, 44 + one.len() as u32);
    put_rsdt(&mut image, RSDT, &[MADT]);

    let mut four = Vec::new();
    for processor in 0..4u8 {
        four.extend_from_slice(&local_apic_entry(processor, processor, ENABLED));
    }
    put_madt(
        &mut image,
        MADT_BEHIND_THE_XSDT,
        0xFEE0_0000,
        &four,
        44 + four.len() as u32,
    );
    put_xsdt(&mut image, XSDT, &[MADT_BEHIND_THE_XSDT]);

    put_rsdp_v2(&mut image, RSDT, XSDT, declared_rsdp_length);
    image
}

/// A MADT whose header DECLARES `declared_length`, whose fixed body header is
/// written whatever that length says, and whose checksum covers exactly the
/// declared bytes. Writing the body header unconditionally is deliberate: a
/// reader that reads it without checking the length finds a real value there,
/// so a test that expects a refusal is not passing because the bytes were
/// absent.
fn put_madt(
    image: &mut Image,
    madt: u64,
    local_apic_address: u32,
    entries: &[u8],
    declared_length: u32,
) {
    put_sdt_header(image, madt, b"APIC", declared_length);
    image.put_u32(madt + 36, local_apic_address);
    image.put_u32(madt + 40, 1); // multiple-APIC flags: PCAT_COMPAT
    if !entries.is_empty() {
        image.put(madt + 44, entries);
    }
    image.seal(madt, declared_length, madt + 9);
}

/// A type-0 Processor Local APIC structure.
fn local_apic_entry(processor_id: u8, apic_id: u8, flags: u32) -> [u8; 8] {
    let mut entry = [0u8; 8];
    entry[0] = 0;
    entry[1] = 8;
    entry[2] = processor_id;
    entry[3] = apic_id;
    entry[4..8].copy_from_slice(&flags.to_le_bytes());
    entry
}

/// A type-9 Processor Local x2APIC structure.
fn local_x2apic_entry(x2apic_id: u32, flags: u32, uid: u32) -> [u8; 16] {
    let mut entry = [0u8; 16];
    entry[0] = 9;
    entry[1] = 16;
    entry[4..8].copy_from_slice(&x2apic_id.to_le_bytes());
    entry[8..12].copy_from_slice(&flags.to_le_bytes());
    entry[12..16].copy_from_slice(&uid.to_le_bytes());
    entry
}

/// Run the reader over `image`, expecting it to produce a census.
fn census_of(image: &Image) -> MadtCensus {
    match read_madt(Some(RSDP), image.physical_memory_offset()) {
        Ok(census) => census,
        Err(refusal) => panic!(
            "expected a census, got the refusal `{}`",
            refusal_token(refusal)
        ),
    }
}

/// Run the reader over `image`, expecting it to refuse.
fn refusal_of(image: &Image) -> MadtRefusal {
    match read_madt(Some(RSDP), image.physical_memory_offset()) {
        Ok(census) => panic!(
            "expected a refusal, got a census of {} processor entries with \
             local_apic_address {:#010x}",
            census.processor_entries, census.local_apic_address
        ),
        Err(refusal) => refusal,
    }
}

/// ANTI-VACUITY: a well-formed MADT is read, entry by entry. This is the
/// baseline the refusal tests below are measured against -- without a test
/// that accepts a table, a reader that refused whatever it was handed would
/// satisfy them.
#[test]
fn a_well_formed_madt_is_counted_entry_by_entry() {
    let mut image = Image::new();
    let mut entries = Vec::new();
    entries.extend_from_slice(&local_apic_entry(0, 0, ENABLED));
    entries.extend_from_slice(&local_apic_entry(1, 1, 0)); // present, not enabled
    entries.extend_from_slice(&local_x2apic_entry(32, ENABLED, 2));

    put_madt(
        &mut image,
        MADT,
        0xFEE0_0000,
        &entries,
        44 + entries.len() as u32,
    );
    put_rsdt(&mut image, RSDT, &[MADT]);
    put_rsdp_v1(&mut image, RSDT);

    let census = census_of(&image);
    assert_eq!(census.processor_entries, 3, "type-0 plus type-9 entries");
    assert_eq!(census.enabled_entries, 2, "entries with flags bit 0 set");
    assert_eq!(census.x2apic_entries, 1, "type-9 entries");
    assert_eq!(census.local_apic_address, 0xFEE0_0000);
    assert_eq!(census.recorded, 3);
    assert_eq!(&census.apic_ids[..3], &[0, 1, 32]);
}

/// The finding this file was written for: a MADT whose declared length does not
/// reach the fixed body header at offsets 36..44 is REFUSED, not read at those
/// offsets. `sdt_length` checksums exactly the declared bytes and vouches for
/// no byte past them, so reading the body header of such a table would read
/// bytes the table does not cover -- here, the sentinel this image writes past
/// the declared end.
#[test]
fn a_madt_too_short_for_its_body_header_is_refused_at_every_such_length() {
    // 36 is the generic SDT minimum `sdt_length` enforces; 44 is the first
    // length that contains the body header. The 8 lengths between the two
    // describe a table that passes the generic check and cannot supply the
    // fields the walk reads.
    for declared_length in 36..44u32 {
        let mut image = Image::new();
        put_madt(
            &mut image,
            MADT,
            SENTINEL_LOCAL_APIC_ADDRESS,
            &[],
            declared_length,
        );
        put_rsdt(&mut image, RSDT, &[MADT]);
        put_rsdp_v1(&mut image, RSDT);

        // The bytes the unguarded read would have taken are present, so the
        // refusal below is a decision and not an empty image.
        assert_eq!(
            image.byte_at(MADT + 36),
            SENTINEL_LOCAL_APIC_ADDRESS.to_le_bytes()[0],
            "the image must carry the body-header bytes a short table does not \
             cover, or this test proves nothing at declared length {declared_length}"
        );

        let refusal = refusal_of(&image);
        assert_eq!(
            refusal_token(refusal),
            "madt_header",
            "a MADT declaring {declared_length} bytes must be refused by its \
             header, and the refusal must be the one the marker reports"
        );
    }
}

/// The partner of the test above: 44 is a floor, not a blanket refusal. A MADT
/// of exactly the minimum length is read, and its body header is what the
/// census reports even though the table carries no entry at all.
#[test]
fn a_madt_of_exactly_the_minimum_length_is_read() {
    let mut image = Image::new();
    put_madt(&mut image, MADT, SENTINEL_LOCAL_APIC_ADDRESS, &[], 44);
    put_rsdt(&mut image, RSDT, &[MADT]);
    put_rsdp_v1(&mut image, RSDT);

    let census = census_of(&image);
    assert_eq!(census.processor_entries, 0, "the table declares no entry");
    assert_eq!(census.enabled_entries, 0);
    assert_eq!(census.recorded, 0);
    assert_eq!(
        census.local_apic_address, SENTINEL_LOCAL_APIC_ADDRESS,
        "the body header is inside a 44-byte table and must be read from it"
    );
}

/// A MADT one byte too short to hold the last entry it points at stops at that
/// entry rather than reading past the declared end -- the length bound on the
/// walk, next to the length bound on the body header above.
#[test]
fn an_entry_that_runs_past_the_declared_length_is_not_counted() {
    let mut image = Image::new();
    let mut entries = Vec::new();
    entries.extend_from_slice(&local_apic_entry(0, 0, ENABLED));
    entries.extend_from_slice(&local_apic_entry(1, 1, ENABLED));

    // Declare one byte short of the second entry's end.
    let declared_length = 44 + entries.len() as u32 - 1;
    put_madt(&mut image, MADT, 0xFEE0_0000, &entries, declared_length);
    put_rsdt(&mut image, RSDT, &[MADT]);
    put_rsdp_v1(&mut image, RSDT);

    let census = census_of(&image);
    assert_eq!(
        census.processor_entries, 1,
        "only the entry the declared length covers is counted"
    );
    assert_eq!(census.recorded, 1);
    assert_eq!(census.apic_ids[0], 0);
}

/// ANTI-VACUITY for the pair below: at the length an ACPI 2.0 RSDP really
/// occupies, the XSDT is followed. Without this, a reader that ignored the
/// XSDT would pass the fallback test for the wrong reason.
#[test]
fn a_full_length_revision_2_rsdp_is_followed_through_its_xsdt() {
    let image = two_root_tables(36);
    let census = census_of(&image);
    assert_eq!(
        census.processor_entries, 4,
        "four processors is the MADT behind the XSDT"
    );
}

/// The second finding this file was written for: a revision >= 2 RSDP whose
/// own Length does not reach the 36 bytes that structure occupies does not
/// contain the XSDT address at offsets 24..32, so the reader falls back to the
/// RSDT rather than trusting a length that cannot cover the field it would
/// read. 33 is where the floor used to sit -- exactly enough to reach the
/// extended checksum byte at offset 32 -- so the loop starts there.
#[test]
fn a_revision_2_rsdp_shorter_than_its_own_structure_falls_back_to_the_rsdt() {
    for declared_length in 33..36u32 {
        let image = two_root_tables(declared_length);
        let census = census_of(&image);
        assert_eq!(
            census.processor_entries, 1,
            "an RSDP declaring {declared_length} bytes must be read through its RSDT: \
             one processor is the MADT behind the RSDT, four would be the XSDT's"
        );
    }
}

#[test]
fn interrupt_topology_keeps_multiple_controllers_overrides_and_64_bit_lapic() {
    let mut image = Image::new();
    let mut entries = Vec::new();
    for (address, gsi) in [(0xfec0_0000u32, 0u32), (0xfec0_1000, 24)] {
        entries.extend_from_slice(&[1, 12, 0, 0]);
        entries.extend_from_slice(&address.to_le_bytes());
        entries.extend_from_slice(&gsi.to_le_bytes());
    }
    for (source, gsi, flags) in [(0u8, 2u32, 0u16), (11, 27, 15)] {
        entries.extend_from_slice(&[2, 10, 0, source]);
        entries.extend_from_slice(&gsi.to_le_bytes());
        entries.extend_from_slice(&flags.to_le_bytes());
    }
    entries.extend_from_slice(&[5, 12, 0, 0]);
    entries.extend_from_slice(&0x1_fee0_0000u64.to_le_bytes());
    put_madt(
        &mut image,
        MADT,
        0xfee0_0000,
        &entries,
        44 + entries.len() as u32,
    );
    put_rsdt(&mut image, RSDT, &[MADT]);
    put_rsdp_v1(&mut image, RSDT);
    let topology = census_of(&image);
    assert_eq!(topology.io_apic_count, 2);
    assert_eq!(topology.io_apics[1].address, 0xfec0_1000);
    assert_eq!(topology.io_apics[1].gsi_base, 24);
    assert_eq!(topology.override_count, 2);
    assert_eq!(topology.overrides[0].source, 0);
    assert_eq!(topology.overrides[0].gsi, 2);
    assert_eq!(topology.overrides[1].flags, 15);
    assert_eq!(topology.local_apic_override, Some(0x1_fee0_0000));
}

#[test]
fn reserved_interrupt_override_flags_are_skipped_without_losing_processors() {
    for flags in [2u16, 8, 16] {
        let mut image = Image::new();
        let mut entries = vec![2, 10, 0, 0, 2, 0, 0, 0];
        entries.extend_from_slice(&flags.to_le_bytes());
        entries.extend_from_slice(&local_apic_entry(0, 0, ENABLED));
        put_madt(
            &mut image,
            MADT,
            0xfee0_0000,
            &entries,
            44 + entries.len() as u32,
        );
        put_rsdt(&mut image, RSDT, &[MADT]);
        put_rsdp_v1(&mut image, RSDT);
        let census = census_of(&image);
        assert_eq!(census.override_count, 0);
        assert_eq!(census.enabled_entries, 1);
        assert!(!census.interrupt_topology_supported);
    }
}

#[test]
fn unsupported_interrupt_entries_do_not_refuse_processor_enumeration() {
    let mut image = Image::new();
    let mut entries = Vec::new();
    for (bus, source) in [(1u8, 0u8), (0, 16)] {
        entries.extend_from_slice(&[2, 10, bus, source, 2, 0, 0, 0, 13, 0]);
    }
    for _ in 0..17 {
        entries.extend_from_slice(&[1, 12, 0, 0, 0, 0, 192, 254, 0, 0, 0, 0]);
        entries.extend_from_slice(&[2, 10, 0, 11, 11, 0, 0, 0, 13, 0]);
    }
    entries.extend_from_slice(&local_apic_entry(7, 7, ENABLED));
    put_madt(
        &mut image,
        MADT,
        0xfee0_0000,
        &entries,
        44 + entries.len() as u32,
    );
    put_rsdt(&mut image, RSDT, &[MADT]);
    put_rsdp_v1(&mut image, RSDT);
    let census = census_of(&image);
    assert_eq!(census.enabled_entries, 1);
    assert_eq!(census.apic_ids[0], 7);
    assert_eq!(census.io_apic_count, 16);
    assert_eq!(census.override_count, 16);
    assert!(!census.interrupt_topology_supported);
}

/// Where a synthetic SPCR sits, as a fake physical address.
const SPCR: u64 = IMAGE_BASE + 0x0400;

/// An SPCR whose header declares `declared_length`, naming a UART of
/// `interface` type at `address` in Generic Address Structure space `space`,
/// with its checksum sealed over the declared bytes.
fn put_spcr(image: &mut Image, interface: u8, space: u8, address: u64, declared_length: u32) {
    put_sdt_header(image, SPCR, b"SPCR", declared_length);
    image.put(SPCR + 36, &[interface]);
    image.put(SPCR + 40, &[space, 8, 0, 1]);
    image.put_u64(SPCR + 44, address);
    image.put(SPCR + 52, &[1, 4]); // interrupt type 8259, IRQ 4
    image.put(SPCR + 58, &[7]); // 115200 baud
    image.seal(SPCR, declared_length, SPCR + 9);
}

/// A MADT and an SPCR behind an RSDT.
fn spcr_behind_the_rsdt(interface: u8, space: u8, address: u64, declared_length: u32) -> Image {
    let mut image = Image::new();
    put_madt(&mut image, MADT, 0xFEE0_0000, &[], 44);
    put_spcr(&mut image, interface, space, address, declared_length);
    put_rsdt(&mut image, RSDT, &[MADT, SPCR]);
    put_rsdp_v1(&mut image, RSDT);
    image
}

fn spcr_port(image: &Image) -> Option<u16> {
    read_spcr_port(Some(RSDP), image.physical_memory_offset())
}

/// ANTI-VACUITY for the refusals below: an SPCR naming a 16550-class UART in
/// I/O space gives that UART's port, for each interface type a 16550 driver
/// serves, behind an RSDT and behind an XSDT.
#[test]
fn an_spcr_naming_a_16550_in_io_space_gives_its_port() {
    for interface in [0x00, 0x01, 0x12] {
        let image = spcr_behind_the_rsdt(interface, 1, 0x3f8, 80);
        assert_eq!(spcr_port(&image), Some(0x3f8), "interface type {interface:#x}");
    }
    let mut image = Image::new();
    put_madt(&mut image, MADT, 0xFEE0_0000, &[], 44);
    put_spcr(&mut image, 0x00, 1, 0x2f8, 80);
    put_xsdt(&mut image, XSDT, &[MADT, SPCR]);
    put_rsdp_v2(&mut image, RSDT, XSDT, 36);
    assert_eq!(spcr_port(&image), Some(0x2f8), "behind the XSDT");
}

/// The console rule falls back to its default for an SPCR it cannot use: a
/// UART in memory space or of another kind, a port 0 or wider than an I/O
/// port, a table too short for its base address or with a bad checksum, and
/// a root table with no SPCR.
#[test]
fn an_spcr_the_console_rule_cannot_use_names_no_port() {
    assert_eq!(spcr_port(&spcr_behind_the_rsdt(0x00, 0, 0xfe03_2000, 80)), None, "memory space");
    assert_eq!(spcr_port(&spcr_behind_the_rsdt(0x03, 1, 0x3f8, 80)), None, "a PL011");
    assert_eq!(spcr_port(&spcr_behind_the_rsdt(0x00, 1, 0, 80)), None, "port 0");
    assert_eq!(spcr_port(&spcr_behind_the_rsdt(0x00, 1, 0x1_0000, 80)), None, "wider than a port");
    assert_eq!(spcr_port(&spcr_behind_the_rsdt(0x00, 1, 0x3f8, 51)), None, "too short");
    let mut image = spcr_behind_the_rsdt(0x00, 1, 0x3f8, 80);
    image.put(SPCR + 70, &[0x5a]);
    assert_eq!(spcr_port(&image), None, "a bad checksum");
    let mut image = Image::new();
    put_madt(&mut image, MADT, 0xFEE0_0000, &[], 44);
    put_rsdt(&mut image, RSDT, &[MADT]);
    put_rsdp_v1(&mut image, RSDT);
    assert_eq!(spcr_port(&image), None, "no SPCR");
    assert_eq!(read_spcr_port(None, image.physical_memory_offset()), None, "no RSDP");
}

const MCFG: u64 = IMAGE_BASE + 0x0500;

fn mcfg_image(base: u64, first: u8, last: u8, length: u32) -> Image {
    let mut image = Image::new();
    put_sdt_header(&mut image, MCFG, b"MCFG", length);
    image.put_u64(MCFG + 44, base);
    image.put(MCFG + 52, &[2, 0, first, last]);
    image.seal(MCFG, length, MCFG + 9);
    put_xsdt(&mut image, XSDT, &[MCFG]);
    put_rsdp_v2(&mut image, RSDT, XSDT, 36);
    image
}

#[test]
fn mcfg_inventory_reports_firmware_apertures_and_refuses_invalid_tables() {
    let image = mcfg_image(0xe0000000, 32, 63, 60);
    let mut entries = Vec::new();
    assert_eq!(acpi::read_mcfg(Some(RSDP), image.physical_memory_offset(), |e| entries.push(e)), Some(1));
    assert_eq!(entries, vec![acpi::McfgAllocation { base: 0xe0000000, segment: 2, first_bus: 32, last_bus: 63 }]);
    for (base, first, last, length) in [(0, 0, 255, 60), (0xe0000001, 0, 255, 60),
        (0xe0000000, 64, 63, 60), (0xe0000000, 0, 255, 43), (0xe0000000, 0, 255, 59),
        (0xfffffffffff00000, 0, 255, 60)] {
        let image = mcfg_image(base, first, last, length);
        assert_eq!(acpi::read_mcfg(Some(RSDP), image.physical_memory_offset(), |_| panic!("published invalid entry")), None);
    }
    let mut image = mcfg_image(0xe0000000, 0, 255, 60);
    image.put(MCFG + 40, &[1]);
    assert_eq!(acpi::read_mcfg(Some(RSDP), image.physical_memory_offset(), |_| panic!("bad checksum")), None);
    assert_eq!(acpi::read_mcfg(None, image.physical_memory_offset(), |_| panic!("absent RSDP")), None);
}

/// A type-4 Local APIC NMI structure.
fn local_apic_nmi_entry(uid: u8, flags: u16, lint: u8) -> [u8; 6] {
    let flags = flags.to_le_bytes();
    [4, 6, uid, flags[0], flags[1], lint]
}

/// A type-10 Local x2APIC NMI structure.
fn local_x2apic_nmi_entry(flags: u16, uid: u32, lint: u8) -> [u8; 12] {
    let mut entry = [0u8; 12];
    entry[0] = 10;
    entry[1] = 12;
    entry[2..4].copy_from_slice(&flags.to_le_bytes());
    entry[4..8].copy_from_slice(&uid.to_le_bytes());
    entry[8] = lint;
    entry
}

fn madt_behind_the_rsdt(entries: &[u8]) -> Image {
    let mut image = Image::new();
    put_madt(&mut image, MADT, 0xfee0_0000, entries, 44 + entries.len() as u32);
    put_rsdt(&mut image, RSDT, &[MADT]);
    put_rsdp_v1(&mut image, RSDT);
    image
}

/// A server-shaped MADT: four enabled processors whose UIDs differ from their
/// APIC ids, disabled placeholders, and one LINT1 NMI structure per UID. Each
/// processor's NMI pin is found through its UID.
#[test]
fn local_nmi_structures_are_matched_to_processors_by_uid() {
    let mut entries = Vec::new();
    for (uid, apic_id) in [(1u8, 0u8), (2, 2), (3, 4), (4, 6)] {
        entries.extend_from_slice(&local_apic_entry(uid, apic_id, ENABLED));
    }
    for uid in 5..=6u8 {
        entries.extend_from_slice(&local_apic_entry(uid, 0xff, 0));
    }
    for uid in 1..=6u8 {
        entries.extend_from_slice(&local_apic_nmi_entry(uid, 0x0005, 1));
    }
    let census = census_of(&madt_behind_the_rsdt(&entries));
    assert_eq!(census.enabled_entries, 4);
    assert_eq!(census.local_nmi_count, 6);
    assert_eq!(&census.apic_uids[..4], &[1, 2, 3, 4]);
    for apic_id in [0, 2, 4, 6] {
        let entry = census.local_nmi_for(apic_id).expect("an NMI pin for every enabled processor");
        assert_eq!((entry.lint, entry.flags), (1, 0x0005), "APIC id {apic_id}");
    }
    assert!(census.local_nmi_for(8).is_none(), "a processor the MADT does not list has no structure");
}

/// A structure naming each processor at once (UID 0xFF, or 0xFFFFFFFF in type
/// 10) applies to each processor, recorded or not; a structure naming a pin other than LINT0
/// or LINT1 is not kept; and the first matching structure wins.
#[test]
fn every_processor_and_x2apic_nmi_structures_apply_and_bad_pins_are_dropped() {
    let mut entries = Vec::new();
    entries.extend_from_slice(&local_apic_entry(0, 0, ENABLED));
    entries.extend_from_slice(&local_apic_nmi_entry(0, 0x000f, 2));
    entries.extend_from_slice(&local_apic_nmi_entry(0xff, 0, 1));
    entries.extend_from_slice(&local_x2apic_nmi_entry(0x000d, u32::MAX, 0));
    let census = census_of(&madt_behind_the_rsdt(&entries));
    assert_eq!(census.local_nmi_count, 2, "the LINT2 structure is dropped");
    assert_eq!(census.local_nmis[0].uid, acpi::ALL_PROCESSORS);
    assert_eq!(census.local_nmis[1].uid, acpi::ALL_PROCESSORS);
    let entry = census.local_nmi_for(0).unwrap();
    assert_eq!((entry.lint, entry.flags), (1, 0));
    let unlisted = census.local_nmi_for(77).unwrap();
    assert_eq!((unlisted.lint, unlisted.flags), (1, 0));

    let x2apic = [local_x2apic_entry(300, ENABLED, 9).as_slice(), &local_x2apic_nmi_entry(0x000f, 9, 0)].concat();
    let census = census_of(&madt_behind_the_rsdt(&x2apic));
    assert_eq!(census.apic_uids[0], 9);
    let entry = census.local_nmi_for(300).unwrap();
    assert_eq!((entry.lint, entry.flags), (0, 0x000f));
}

#[test]
fn the_table_census_lists_the_root_table_then_each_table_in_order() {
    let mut image = spcr_behind_the_rsdt(0x00, 1, 0x3f8, 80);
    let mut signatures = Vec::new();
    let visited = acpi::visit_table_signatures(Some(RSDP), image.physical_memory_offset(), |signature| {
        signatures.push(signature)
    });
    assert_eq!(visited, Some(3));
    assert_eq!(signatures, vec![*b"RSDT", *b"APIC", *b"SPCR"]);
    // A root table with a bad checksum gives no census.
    image.put(RSDT + 30, &[0x77]);
    assert_eq!(acpi::visit_table_signatures(Some(RSDP), image.physical_memory_offset(), |_| {}), None);
    assert_eq!(acpi::visit_table_signatures(None, image.physical_memory_offset(), |_| {}), None);
}

const FADT: u64 = IMAGE_BASE + 0x0600;

/// A FADT of `length` bytes with a PM timer at I/O port `pm_port`, `flags`,
/// IA-PC boot flags `boot_arch`, the reset register at I/O 0xcf9 with value 6,
/// and the extended timer block at `x_pm` when the table reaches it.
fn fadt_image(length: u32, pm_port: u32, flags: u32, boot_arch: u16, x_pm: u64) -> Image {
    let mut image = Image::new();
    put_sdt_header(&mut image, FADT, b"FACP", length);
    image.put(FADT + 46, &9u16.to_le_bytes());
    image.put_u32(FADT + 76, pm_port);
    image.put(FADT + 91, &[4]);
    image.put(FADT + 109, &boot_arch.to_le_bytes());
    image.put_u32(FADT + 112, flags);
    if length > 128 {
        image.put(FADT + 116, &[1, 8, 0, 1]);
        image.put_u64(FADT + 120, 0xcf9);
        image.put(FADT + 128, &[6]);
    }
    if length >= 220 {
        image.put(FADT + 208, &[1, 32, 0, 3]);
        image.put_u64(FADT + 212, x_pm);
    }
    image.seal(FADT, length, FADT + 9);
    put_rsdt(&mut image, RSDT, &[FADT]);
    put_rsdp_v1(&mut image, RSDT);
    image
}

fn fadt_of(image: &Image) -> Option<acpi::Fadt> {
    acpi::read_fadt(Some(RSDP), image.physical_memory_offset())
}

#[test]
fn the_fadt_gives_its_pm_timer_reset_register_and_boot_flags() {
    // An ACPI 6 table: the extended timer block wins over the 32-bit one.
    let fadt = fadt_of(&fadt_image(276, 0x408, (1 << 8) | (1 << 10), 0, 0x508)).unwrap();
    assert_eq!(fadt.sci_interrupt, 9);
    assert_eq!(fadt.pm_timer.map(|gas| (gas.space, gas.address)), Some((1, 0x508)));
    assert!(fadt.pm_timer_32bit);
    assert_eq!(fadt.reset.map(|(gas, value)| (gas.space, gas.address, value)), Some((1, 0xcf9, 6)));
    assert!(!fadt.msi_not_supported() && !fadt.hardware_reduced());

    // An extended block of address 0 falls back to PM_TMR_BLK; no RESET_REG_SUP, no reset.
    let fadt = fadt_of(&fadt_image(276, 0x608, 0, 1 << 3, 0)).unwrap();
    assert_eq!(fadt.pm_timer.map(|gas| (gas.space, gas.address)), Some((1, 0x608)));
    assert!(!fadt.pm_timer_32bit);
    assert_eq!(fadt.reset, None);
    assert!(fadt.msi_not_supported());

    // An ACPI 1.0 table (116 bytes) has a timer and no reset register.
    let fadt = fadt_of(&fadt_image(116, 0x608, 1 << 10, 0, 0)).unwrap();
    assert_eq!(fadt.pm_timer.map(|gas| gas.address), Some(0x608));
    assert_eq!(fadt.reset, None);

    // No timer at all, and a hardware-reduced platform.
    let fadt = fadt_of(&fadt_image(276, 0, 1 << 20, 0, 0)).unwrap();
    assert_eq!(fadt.pm_timer, None);
    assert!(fadt.hardware_reduced());
}

#[test]
fn a_fadt_too_short_or_with_a_bad_checksum_is_refused() {
    assert_eq!(fadt_of(&fadt_image(115, 0x608, 0, 0, 0)), None, "shorter than ACPI 1.0");
    let mut image = fadt_image(276, 0x608, 0, 0, 0);
    image.put(FADT + 200, &[0x42]);
    assert_eq!(fadt_of(&image), None, "a bad checksum");
    let image = spcr_behind_the_rsdt(0x00, 1, 0x3f8, 80);
    assert_eq!(fadt_of(&image), None, "no FADT");
}

#[test]
fn nmi_entries_cover_the_full_processor_capacity() {
    let mut entries = Vec::new();
    for cpu in 0..acpi::MAX_ENUMERATED_CPUS as u8 {
        entries.extend_from_slice(&local_apic_entry(cpu, cpu, ENABLED));
        entries.extend_from_slice(&local_apic_nmi_entry(cpu, 0, 1));
    }
    let census = census_of(&madt_behind_the_rsdt(&entries));
    assert_eq!(census.local_nmi_count, acpi::MAX_ENUMERATED_CPUS);
    for cpu in 0..acpi::MAX_ENUMERATED_CPUS as u32 {
        assert!(census.local_nmi_for(cpu).is_some(), "missing NMI pin for CPU {cpu}");
    }
}

fn kcs_firmware(v3: bool, spacing: u8) -> Image {
    let mut image = Image::new();
    let handoff = IMAGE_BASE + 0x400;
    let entry = IMAGE_BASE + 0x500;
    let table = IMAGE_BASE + 0x600;
    put_rsdp_v1(&mut image, RSDT);
    put_rsdt(&mut image, RSDT, &[handoff]);
    put_sdt_header(&mut image, handoff, b"UEFI", 62);
    image.put(handoff + 36, &[0x0b,0x76,0xb1,0x32,0x04,0x68,0x9c,0x4f,0xab,0x21,0x67,0xf5,0x8f,0x56,0x42,0xe1]);
    image.put(handoff + 52, &54u16.to_le_bytes());
    image.put_u64(handoff + 54, entry);
    image.seal(handoff, 62, handoff + 9);
    if v3 {
        image.put(entry, b"_SM3_");
        image.put(entry + 6, &[24]);
        image.put_u32(entry + 12, 26);
        image.put_u64(entry + 16, table);
        image.seal(entry, 24, entry + 5);
    } else {
        image.put(entry, b"_SM_");
        image.put(entry + 5, &[31]);
        image.put(entry + 16, b"_DMI_");
        image.put(entry + 22, &26u16.to_le_bytes());
        image.put_u32(entry + 24, table as u32);
        image.seal(entry + 16, 15, entry + 21);
        image.seal(entry, 31, entry + 4);
    }
    image.put(table, &[38,18,0,0,1,0x20,0x20,0xff]);
    image.put_u64(table + 8, 0x701); // synthetic I/O base 0x700
    image.put(table + 16, &[spacing << 6, 0]);
    image.put(table + 20, &[127,4,0,0,0,0]);
    image
}

#[test]
fn kcs_uses_smbios_entry_point_and_register_spacing() {
    for v3 in [false, true] {
        for (spacing, stride) in [(0,1),(1,4),(2,16)] {
            let image = kcs_firmware(v3, spacing);
            assert_eq!(acpi::read_kcs_ports(Some(RSDP), image.physical_memory_offset()), Some((0x700, 0x700 + stride)));
        }
    }
}

#[test]
fn kcs_refuses_bad_entry_checksum_and_truncated_structure() {
    let mut image = kcs_firmware(true, 0);
    image.put(IMAGE_BASE + 0x500 + 5, &[1]);
    assert_eq!(acpi::read_kcs_ports(Some(RSDP), image.physical_memory_offset()), None);
    let mut image = kcs_firmware(true, 0);
    image.put_u32(IMAGE_BASE + 0x500 + 12, 8);
    image.seal(IMAGE_BASE + 0x500, 24, IMAGE_BASE + 0x500 + 5);
    assert_eq!(acpi::read_kcs_ports(Some(RSDP), image.physical_memory_offset()), None);
}
