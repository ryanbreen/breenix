#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub struct Time {
    pub seconds: u64,
    pub millis: u64,
    pub nanos: u64,
}

impl Time {
    pub const fn new(seconds: u64, millis: u64, nanos: u64) -> Self {
        Self {
            seconds,
            millis,
            nanos,
        }
    }

    pub const fn from_seconds(seconds: u64) -> Self {
        Self {
            seconds,
            millis: 0,
            nanos: 0,
        }
    }

    pub const fn from_millis(total_millis: u64) -> Self {
        let seconds = total_millis / 1000;
        let millis = total_millis % 1000;
        Self {
            seconds,
            millis,
            nanos: 0,
        }
    }

    pub const fn total_millis(&self) -> u64 {
        self.seconds * 1000 + self.millis + self.nanos / 1_000_000
    }

    pub const fn total_nanos(&self) -> u128 {
        self.seconds as u128 * 1_000_000_000 + self.millis as u128 * 1_000_000 + self.nanos as u128
    }
}

impl core::fmt::Display for Time {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}s {}ms {}ns", self.seconds, self.millis, self.nanos)
    }
}
