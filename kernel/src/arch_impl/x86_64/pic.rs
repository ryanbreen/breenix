use crate::arch_impl::traits::InterruptController;
use crate::interrupts::{PICS, PIC_1_OFFSET};

pub struct X86Pic;

impl InterruptController for X86Pic {
    fn init() {}

    fn enable_irq(irq: u8) {
        let mut pics = PICS.lock();
        unsafe {
            let mut masks = pics.read_masks();
            if irq < 8 {
                masks[0] &= !(1u8 << irq);
            } else {
                masks[1] &= !(1u8 << (irq - 8));
            }
            pics.write_masks(masks[0], masks[1]);
        }
    }

    fn disable_irq(irq: u8) {
        let mut pics = PICS.lock();
        unsafe {
            let mut masks = pics.read_masks();
            if irq < 8 {
                masks[0] |= 1u8 << irq;
            } else {
                masks[1] |= 1u8 << (irq - 8);
            }
            pics.write_masks(masks[0], masks[1]);
        }
    }

    fn send_eoi(vector: u8) {
        let mut pics = PICS.lock();
        unsafe {
            pics.notify_end_of_interrupt(vector);
        }
    }

    fn irq_offset() -> u8 {
        PIC_1_OFFSET
    }
}
