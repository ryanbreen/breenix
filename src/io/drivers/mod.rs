pub mod network;

pub trait DeviceDriver {
    fn initialize(&mut self);
}
