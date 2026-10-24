//! A health round may feed only after I/O succeeds. Readback detects a BMC
//! restart or lost configuration before the next reset command.
pub const TIMER: [u8; 6] = [4, 1, 0, 0x10, 0xb0, 4];

pub trait Transport {
    fn get(&mut self) -> Result<[u8; 8], &'static str>;
    fn set(&mut self) -> Result<(), &'static str>;
    fn reset(&mut self) -> Result<(), &'static str>;
}
fn configured(timer: &[u8; 8]) -> bool {
    timer[0] & 0x47 == 0x44 && timer[1] == 1 && timer[2] == 0 && timer[4..6] == TIMER[4..6]
}
pub fn feed(transport: &mut impl Transport, health: impl FnOnce() -> Result<(), &'static str>) -> Result<bool, &'static str> {
    health()?;
    let rearm = !configured(&transport.get()?);
    if rearm { transport.set()?; }
    transport.reset()?;
    if !configured(&transport.get()?) { return Err("watchdog settings readback"); }
    Ok(rearm)
}
