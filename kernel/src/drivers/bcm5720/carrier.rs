//! Runtime carrier filtering after the port has negotiated a link.
//!
//! BMSR link status is latch-low; phy_link() already reads it twice (IEEE
//! 802.3 clause 22). An MDIO failure supplies no link observation. Three
//! successful down samples at the worker's >=1 s cadence span >=2 s, beyond
//! clause 28's 1200-1500 ms break_link_timer during autonegotiation restart.
//! This filters that transient, not an arbitrarily long negotiation. Up is
//! immediate; an unknown sample breaks the consecutive-down run.

pub const DOWN_SAMPLES: u8 = 3;

pub struct Carrier {
    pub up: bool,
    pub failures: u64,
    down: u8,
}

impl Carrier {
    pub const fn linked() -> Self {
        Self {
            up: true,
            failures: 0,
            down: 0,
        }
    }

    pub fn sample(&mut self, sample: Result<bool, &'static str>) {
        match sample {
            Err(_) => {
                self.failures = self.failures.saturating_add(1);
                self.down = 0;
            }
            Ok(up) => {
                self.failures = 0;
                self.down = if up { 0 } else { self.down.saturating_add(1) };
                if up || self.down >= DOWN_SAMPLES {
                    self.up = up;
                }
            }
        }
    }
}
