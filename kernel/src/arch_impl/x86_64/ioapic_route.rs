//! Pure ISA redirection construction; kept separate for host execution tests.
pub const MASKED: u32 = 1 << 16;

/// ISA conforming flags mean edge/high; ELCR applies when no override exists.
pub fn redirection(
    irq: u8,
    destination: u8,
    elcr_level: bool,
    source_override: Option<(u32, u16)>,
) -> (u32, u32, u32) {
    let mut gsi = u32::from(irq);
    let mut low = u32::from(32 + irq) | MASKED;
    if let Some((override_gsi, flags)) = source_override {
        gsi = override_gsi;
        if flags & 3 == 3 {
            low |= 1 << 13;
        }
        if (flags >> 2) & 3 == 3 {
            low |= 1 << 15;
        }
    } else if elcr_level {
        low |= 1 << 15;
    }
    (gsi, low, u32::from(destination) << 24)
}
