//! x86_64 PIC (8259A) interrupt controller.
//!
//! Note: This is part of the complete HAL API. The X86Pic struct
//! implements the InterruptController trait.

#![allow(dead_code)] // HAL type - part of complete API

use crate::arch_impl::traits::InterruptController;
use crate::interrupts::PIC_1_OFFSET;

pub struct X86Pic;

impl InterruptController for X86Pic {
    fn init() {}

    fn enable_irq(irq: u8) {
        super::irq::set_enabled(irq, true);
    }

    fn disable_irq(irq: u8) {
        super::irq::set_enabled(irq, false);
    }

    fn send_eoi(vector: u8) {
        super::irq::eoi(vector);
    }

    fn irq_offset() -> u8 {
        PIC_1_OFFSET
    }
}
