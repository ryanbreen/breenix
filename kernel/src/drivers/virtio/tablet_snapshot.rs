//! Coherent tablet frames and per-process legacy polling gesture boundaries.
//! The driver serializes operations with interrupts masked. Event-stream
//! publication is independent of this bounded snapshot interface.
#[derive(Clone, Copy)]
struct Reader {
    owner: u64,
    used: u64,
    released: Option<(u32, u32, u32)>,
    pressed: Option<(u32, u32, u32)>,
    tracking: bool,
}
const EMPTY: Reader = Reader {
    owner: 0,
    used: 0,
    released: None,
    pressed: None,
    tracking: false,
};
pub struct TabletSnapshot {
    current: (u32, u32, u32),
    readers: [Reader; 16],
    active: usize,
    clock: u64,
}
impl TabletSnapshot {
    pub const fn new() -> Self {
        Self {
            current: (0, 0, 0),
            readers: [EMPTY; 16],
            active: 0,
            clock: 0,
        }
    }
    /// Publish once, at SYN_REPORT, including axes that followed button-up.
    pub fn report(&mut self, x: u32, y: u32, buttons: u32) {
        let next = (x, y, buttons);
        for (index, reader) in self.readers.iter_mut().enumerate() {
            if index != 0 && reader.owner == 0 {
                continue;
            }
            if reader.tracking && buttons & !self.current.2 != 0 && reader.pressed.is_none() {
                reader.pressed = Some(next);
            }
            if self.current.2 & !buttons != 0 && reader.released.is_none() {
                reader.released = Some(next);
            }
        }
        self.current = next;
    }
    pub fn current(&self) -> (u32, u32, u32) {
        self.current
    }
    pub fn take(&mut self) -> (u32, u32, u32) {
        let reader = &mut self.readers[self.active];
        reader.tracking = true;
        if let Some(press) = reader.pressed.take() {
            if reader.released.is_some() {
                return press;
            }
        }
        reader.released.take().unwrap_or(self.current)
    }
    /// A new process starts at the current level with an empty release history. Active readers keep independent boundaries. Kernel
    /// process IDs are not reused within a boot; the least recently used slot
    /// is reclaimed when the bounded legacy observer table fills.
    pub fn take_for(&mut self, owner: u64) -> (u32, u32, u32) {
        self.clock += 1;
        self.active = (1..self.readers.len())
            .find(|i| self.readers[*i].owner == owner)
            .unwrap_or_else(|| {
                let at = (1..self.readers.len())
                    .min_by_key(|i| self.readers[*i].used)
                    .unwrap();
                self.readers[at] = Reader { owner, ..EMPTY };
                at
            });
        self.readers[self.active].used = self.clock;
        self.take()
    }
    /// Position-only control, under the same IRQ-masked lock as report. It
    /// cannot resurrect a button or leave a stale gesture boundary.
    pub fn move_to(&mut self, x: u32, y: u32) {
        self.current.0 = x;
        self.current.1 = y;
        for reader in &mut self.readers {
            reader.released = None;
            reader.pressed = None;
        }
    }
}
