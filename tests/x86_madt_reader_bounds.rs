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

use acpi::{read_madt, refusal_token, MadtCensus, MadtRefusal};

/// The fake physical address the synthetic image's first byte stands at. Well
/// below the reader's `PHYS_READ_CEILING`, so a refusal in this file is a
/// length decision rather than a ceiling one.
const IMAGE_BASE: u64 = 0x0002_0000;

/// Bytes of synthetic physical memory: room for the 5 tables laid out below.
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
