//! Panic text geometry for the retained target, chosen during preparation.
#[derive(Clone, Copy)]
pub(crate) struct Layout {
    pub scale: u32,
    pub cell_width: u32,
    pub cell_height: u32,
}

impl Layout {
    pub fn new(glyph_width: u32, width: u32, height: u32) -> Result<Self, &'static str> {
        for scale in [2, 1] {
            let cell_width = glyph_width.checked_mul(scale).ok_or("panic glyph width")?;
            let cell_height = 23 * scale;
            if glyph_width != 0
                && cell_width.checked_mul(80).and_then(|n| n.checked_add(40)).is_some_and(|n| n <= width)
                && 32 + 31 * cell_height + 16 * scale <= height
            {
                return Ok(Self { scale, cell_width, cell_height });
            }
        }
        Err("panic grid exceeds target")
    }
}
