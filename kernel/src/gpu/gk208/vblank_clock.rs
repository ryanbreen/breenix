//! G33a presentation time on the native scanout timeline.
//! The epoch is one observed vblank edge. Accumulate the hardware frame count,
//! then convert the total with the exact programmed pixel clock and totals;
//! rounding each period separately would accumulate error. Call at least once
//! per 16-bit counter wrap (over 18 minutes at 60 Hz).
pub struct VblankClock {
    last: u16,
    frames: u64,
    epoch_ns: u64,
    frame_pixels: u64,
    pixel_hz: u64,
}
impl VblankClock {
    pub fn new(count: u16, at: u64, frame_pixels: u64, pixel_hz: u64) -> Self {
        Self { last: count, frames: 0, epoch_ns: at, frame_pixels, pixel_hz }
    }
    pub fn at(&mut self, count: u16) -> u64 {
        self.frames += u64::from(count.wrapping_sub(self.last));
        self.last = count;
        self.epoch_ns + (u128::from(self.frames) * u128::from(self.frame_pixels)
            * 1_000_000_000 / u128::from(self.pixel_hz)) as u64
    }
}
