pub mod network;
pub mod qemu;

pub trait DeviceDriver {
    fn initialize(&mut self) -> Result<(), ()>;
}
