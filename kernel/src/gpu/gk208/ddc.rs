//! NXP UM10204 I2C and VESA E-DDC, with bounded stretching and atomic block output.
pub trait Pins {
    /// true releases an open-drain line; false pulls it low.
    fn drive(&mut self, scl: bool, sda: bool);
    fn sense(&mut self) -> (bool, bool);
    fn delay(&mut self);
}
fn high(p: &mut impl Pins, sda: bool) -> Result<(), &'static str> {
    p.drive(true, sda);
    for _ in 0..512 {
        p.delay();
        if p.sense().0 {
            return Ok(());
        }
    }
    Err("DDC clock stretch timeout")
}
fn start(p: &mut impl Pins) -> Result<(), &'static str> {
    high(p, true)?;
    if !p.sense().1 {
        return Err("DDC bus busy");
    }
    p.drive(true, false);
    p.delay();
    p.drive(false, false);
    p.delay();
    Ok(())
}
fn bit(p: &mut impl Pins, value: bool) -> Result<bool, &'static str> {
    p.drive(false, value);
    p.delay();
    high(p, value)?;
    let b = p.sense().1;
    p.drive(false, value);
    p.delay();
    Ok(b)
}
fn write(p: &mut impl Pins, value: u8) -> Result<(), &'static str> {
    for n in (0..8).rev() {
        bit(p, value & (1 << n) != 0)?;
    }
    if bit(p, true)? {
        Err("DDC NACK")
    } else {
        Ok(())
    }
}
fn stop(p: &mut impl Pins) {
    p.drive(false, false);
    p.delay();
    let _ = high(p, false);
    p.drive(true, true);
    p.delay();
}
pub fn read_block(
    p: &mut impl Pins,
    block: u8,
    output: &mut [u8; 128],
) -> Result<(), &'static str> {
    let mut data = [0u8; 128];
    let result = (|| {
        if block > 3 {
            return Err("DDC extension bound");
        }
        if block >= 2 {
            start(p)?;
            write(p, 0x60)?;
            write(p, block / 2)?;
        }
        start(p)?;
        write(p, 0xa0)?;
        write(p, (block % 2) * 128)?;
        start(p)?;
        write(p, 0xa1)?;
        for (i, b) in data.iter_mut().enumerate() {
            for _ in 0..8 {
                *b = (*b << 1) | u8::from(bit(p, true)?);
            }
            bit(p, i == 127)?;
        }
        Ok(())
    })();
    stop(p);
    if result.is_ok() {
        *output = data;
    }
    result
}
