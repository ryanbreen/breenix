pub mod display;
pub mod network;
pub mod qemu;

pub trait DeviceDriver {
    fn initialize(&mut self);
}
