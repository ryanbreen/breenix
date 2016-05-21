pub mod network;

trait DeviceDriver {
    fn initialize(&mut self);
}