extern crate alloc;
#[path = "../../../kernel/src/graphics/boot_ribbon.rs"]
pub mod boot_ribbon;
#[path = "../../../kernel/src/graphics/boot_producer.rs"]
pub mod boot_producer;
pub mod graphics {
    pub use crate::{boot_producer, boot_ribbon};
}
pub fn arch_interrupts_enabled() -> bool { true }
