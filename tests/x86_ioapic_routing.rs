#[path = "../kernel/src/arch_impl/x86_64/ioapic_route.rs"]
mod ioapic_route;
use ioapic_route::{redirection, MASKED};

#[test]
fn qemu_pci_overrides_are_high_level_even_without_elcr() {
    for irq in [5, 9, 10, 11] {
        for elcr in [false, true] {
            let (gsi, low, high) = redirection(irq, 0, elcr, Some((u32::from(irq), 0xd)));
            assert_eq!(gsi, u32::from(irq));
            assert_eq!(low, MASKED | (1 << 15) | u32::from(32 + irq));
            assert_eq!(high, 0);
        }
    }
}

#[test]
fn elcr_applies_only_without_an_override() {
    assert_eq!(
        redirection(11, 7, true, None),
        (11, MASKED | (1 << 15) | 43, 7 << 24)
    );
    assert_eq!(redirection(11, 7, false, None), (11, MASKED | 43, 7 << 24));
    assert_eq!(
        redirection(11, 7, true, Some((27, 0))),
        (27, MASKED | 43, 7 << 24)
    );
    assert_eq!(
        redirection(11, 7, false, Some((27, 15))),
        (27, MASKED | (1 << 13) | (1 << 15) | 43, 7 << 24)
    );
}

#[test]
fn pit_override_stays_masked_and_uses_its_gsi() {
    assert_eq!(redirection(0, 0, false, Some((2, 0))), (2, MASKED | 32, 0));
}
