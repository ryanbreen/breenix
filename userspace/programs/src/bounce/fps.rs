pub struct FpsCounter {
    last_time_ms: u64,
    frame_count: u32,
    pub display_fps: u32,
}

impl FpsCounter {
    pub fn new() -> Self {
        Self { last_time_ms: 0, frame_count: 0, display_fps: 0 }
    }

    pub fn tick(&mut self, now: u64) {
        self.frame_count += 1;
        if self.frame_count >= 16 {
            let elapsed = now.saturating_sub(self.last_time_ms);
            if elapsed > 0 {
                self.display_fps = (self.frame_count as u64 * 1_000 / elapsed) as u32;
            }
            self.frame_count = 0;
            self.last_time_ms = now;
        }
    }
}
