//! The x86 MSI message (`kernel/src/arch_impl/x86_64/msi_message.rs`),
//! executed against the address and data layout of the Intel SDM vol. 3A.

#[path = "../kernel/src/arch_impl/x86_64/msi_message.rs"]
mod msi_message;

use msi_message::compose;

#[test]
fn the_message_addresses_one_physical_destination_with_a_fixed_edge_vector() {
    assert_eq!(compose(0x50, 0), Some((0xfee0_0000, 0x50)));
    assert_eq!(compose(0x5f, 6), Some((0xfee0_6000, 0x5f)));
    let (address, data) = compose(0x51, 0xff).unwrap();
    assert_eq!(address >> 20, 0xfee, "the interrupt address range");
    assert_eq!((address >> 12) & 0xff, 0xff, "destination id in bits 19:12");
    assert_eq!(address & 0b1100, 0, "no redirection hint, physical destination mode");
    assert_eq!(data & !0xff, 0, "fixed delivery, edge trigger");
}

#[test]
fn destinations_and_vectors_the_message_cannot_carry_are_refused() {
    assert_eq!(compose(0x50, 0x100), None, "an APIC id above the 8-bit field");
    assert_eq!(compose(0x1f, 0), None, "an exception vector");
    assert_eq!(compose(0xff, 0), None, "the spurious vector");
}
