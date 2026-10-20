//! The address and data of an x86 message-signalled interrupt. Pure, so the
//! host executes it (`tests/x86_msi_message.rs`).
//!
//! From the Intel SDM vol. 3A, "Message Signalled Interrupts": the address is
//! 0xFEE in bits 31:20, the destination APIC id in bits 19:12, redirection hint
//! (bit 3) and destination mode (bit 2) clear for one physical destination;
//! the data is the vector in bits 7:0, fixed delivery (bits 10:8 clear) and an
//! edge trigger (bit 15 clear). The 8-bit destination field reaches APIC ids
//! below 256 only, in xAPIC and x2APIC mode alike, without interrupt remapping.

/// The address and data for `vector` at the processor with APIC id
/// `destination`; no value when the destination does not fit the field or the
/// vector is not a deliverable fixed vector.
pub fn compose(vector: u8, destination: u32) -> Option<(u32, u32)> {
    if destination > 0xff || vector < 32 || vector == 0xff {
        return None;
    }
    Some((0xfee0_0000 | (destination << 12), u32::from(vector)))
}
