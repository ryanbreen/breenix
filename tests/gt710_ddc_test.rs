//! I2C error-path check independent of the MMIO implementation (NXP UM10204).
#[path = "../kernel/src/gpu/gk208/ddc.rs"]
mod ddc;

struct Pins { stuck_clock: bool, ticks: usize, released: bool }
impl ddc::Pins for Pins {
    fn drive(&mut self, scl: bool, sda: bool) { self.released = scl && sda; }
    fn sense(&mut self) -> (bool, bool) { (!self.stuck_clock, true) }
    fn delay(&mut self) { self.ticks += 1; }
}

#[test]
fn nack_and_clock_stretch_timeout_cannot_yield_edid_bytes() {
    for stuck in [false, true] {
        let mut pins = Pins { stuck_clock: stuck, ticks: 0, released: false };
        let mut bytes = [0xa5;128];
        assert!(ddc::read_block(&mut pins, 0, &mut bytes).is_err());
        assert!(pins.ticks <= 5000, "unbounded clock-stretch polling");
        assert!(pins.released, "error path leaves the bus driven");
        assert_eq!(bytes, [0xa5;128]);
    }
}
