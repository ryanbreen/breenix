pub fn script(name: &str) -> Option<&'static [u8]> {
    match name {
        "vt-screen" => Some(b"\x1b[2J\x1b[HHello \x1b[31mred\x1b[0m!\x1b[2;1HmovedX\x08\x1b[K"),
        "scroll-region" => Some(b"\x1b[2J\x1b[Htop\x1b[2;1Hone\x1b[3;1Htwo\x1b[4;1Hthree\x1b[5;1Hbottom\x1b[2;4r\x1b[4;1H\x1bD\x1b[2;1H\x1bM\x1b[1L\x1b[1M\x1b[r\x1b[2;1Htwo\x1b[3;1Hthree\x1b[4;1Hbottom\x1b[5;1H\x1b[K"),
        "alt-screen" => Some(b"\x1b[2J\x1b[Hmain restored\x1b[?1049h\x1b[2J\x1b[Halternate\x1b[?1049l"),
        "utf8" => Some(b"\x1b[2J\x1b[HA\xc3\xa9\xe7\x95\x8c\xffB"),
        _ => None,
    }
}

/// Keep the screen stable until the PTY supplies input or closes.
pub fn write_and_hold(input: &mut impl std::io::Read, output: &mut impl std::io::Write, bytes: &[u8]) -> std::io::Result<()> {
    for byte in bytes {
        output.write_all(&[*byte])?;
        output.flush()?;
    }
    input.read(&mut [0u8; 1])?;
    Ok(())
}
