//! Coherent tablet frames and per-process legacy polling gesture boundaries.
//! The driver serializes operations with interrupts masked. Event-stream
//! publication is independent of this bounded snapshot interface.
const BOUNDARIES: usize = 8;
#[derive(Clone, Copy)]
struct Boundary {
    state: (u32, u32, u32),
    press: bool,
}
const ZERO: Boundary = Boundary {
    state: (0, 0, 0),
    press: false,
};
#[derive(Clone, Copy)]
struct Reader {
    owner: u64,
    used: u64,
    boundaries: [Boundary; BOUNDARIES],
    head: usize,
    len: usize,
    tracking: bool,
}
const EMPTY: Reader = Reader {
    owner: 0,
    used: 0,
    boundaries: [ZERO; BOUNDARIES],
    head: 0,
    len: 0,
    tracking: false,
};
impl Reader {
    fn push(&mut self, boundary: Boundary) {
        // This level-snapshot ABI has bounded history. On overflow, resume at
        // the newest boundary; lossless consumers use the queued input ABI.
        if self.len == BOUNDARIES {
            self.head = 0;
            self.len = 0;
        }
        self.boundaries[(self.head + self.len) % BOUNDARIES] = boundary;
        self.len += 1;
    }
    fn pop(&mut self) -> Option<Boundary> {
        if self.len == 0 {
            return None;
        }
        let boundary = self.boundaries[self.head];
        self.head = (self.head + 1) % BOUNDARIES;
        self.len -= 1;
        Some(boundary)
    }
}
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
    /// Update a boundary at the completed frame's coordinates. The caller
    /// holds the snapshot lock through the frame, including axes after button-up.
    pub fn report(&mut self, x: u32, y: u32, buttons: u32) {
        let previous = self.current.2;
        self.current = (x, y, buttons);
        if buttons == previous {
            return;
        }
        let press = buttons & !previous != 0;
        let release = previous & !buttons != 0;
        for (index, reader) in self.readers.iter_mut().enumerate() {
            if index != 0 && reader.owner == 0 {
                continue;
            }
            if reader.tracking || release {
                reader.push(Boundary {
                    state: self.current,
                    press,
                });
            }
        }
    }
    /// Fold button edges at the completed frame's coordinates while the
    /// driver owns the snapshot lock. Intermediate levels update the latches
    /// without exposing a partial ABS/KEY frame to readers.
    pub fn frame(&mut self, x: u32, y: u32, edges: impl Iterator<Item = (u16, i32)>) {
        let mut buttons = self.current.2;
        for (code, value) in edges {
            let bit = 1 << (code - 272);
            if value == 0 {
                buttons &= !bit;
            } else {
                buttons |= bit;
            }
            self.report(x, y, buttons);
        }
        self.report(x, y, buttons);
    }
    pub fn current(&self) -> (u32, u32, u32) {
        self.current
    }
    pub fn take(&mut self) -> (u32, u32, u32) {
        let reader = &mut self.readers[self.active];
        reader.tracking = true;
        let Some(mut boundary) = reader.pop() else {
            return self.current;
        };
        if boundary.press {
            // Added buttons can coalesce to their combined level. A release
            // ends that run: a later press cannot overtake an older release.
            while reader.len != 0 && reader.boundaries[reader.head].press {
                boundary = reader.pop().unwrap();
            }
            if reader.len == 0 && self.current.2 == boundary.state.2 {
                return self.current;
            }
        }
        boundary.state
    }
    /// A new process starts at the current level with an empty release history.
    /// Active readers keep independent boundaries. Kernel process IDs are not
    /// reused within a boot; the least recently used slot is reclaimed when
    /// the bounded legacy observer table fills.
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
            reader.head = 0;
            reader.len = 0;
        }
    }
}
