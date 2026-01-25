//! ARM64 GICv2 (Generic Interrupt Controller) interrupt controller.

#![allow(dead_code)]

use crate::arch_impl::traits::InterruptController;

pub struct Gicv2;

impl InterruptController for Gicv2 {
    fn init() {
        unimplemented!("ARM64: GICv2 init not yet implemented")
    }

    fn enable_irq(irq: u8) {
        let _ = irq;
        unimplemented!("ARM64: GICv2 enable_irq not yet implemented")
    }

    fn disable_irq(irq: u8) {
        let _ = irq;
        unimplemented!("ARM64: GICv2 disable_irq not yet implemented")
    }

    fn send_eoi(vector: u8) {
        let _ = vector;
        unimplemented!("ARM64: GICv2 send_eoi not yet implemented")
    }

    fn irq_offset() -> u8 {
        32 // SPIs start at 32 on ARM GIC
    }
}
