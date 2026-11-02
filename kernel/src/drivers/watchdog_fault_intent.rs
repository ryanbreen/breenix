#[derive(Debug, PartialEq, Eq)]
pub enum Mode {
    FeederHang,
    HealthHang,
}

pub fn parse(bytes: &[u8]) -> Result<Option<Mode>, &'static str> {
    if bytes.len() >  32 { return Err("fault intent too long"); }
    match bytes {
        b"feeder-hang\n" => Ok(Some(Mode::FeederHang)),
        b"health-hang\n" => Ok(Some(Mode::HealthHang)),
        b"" => Ok(None),
        _ => Err("fault intent invalid"),
    }
}
