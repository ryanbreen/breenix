//! Lock-free PIC acknowledgement prerequisite for controller dispatch.
use x86_64::instructions::port::Port;

#[inline]
pub fn eoi(vector: u8) {
        unsafe {
            if let Some(mut pics) = crate::interrupts::PICS.try_lock() {
                pics.notify_end_of_interrupt(vector);
            } else if (32..48).contains(&vector) {
                if vector >= 40 {
                    Port::<u8>::new(0xa0).write(0x20);
                }
                Port::<u8>::new(0x20).write(0x20);
            }
        }
}
