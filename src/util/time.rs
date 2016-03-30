use core::fmt;

/// A point in time
#[derive(Copy, Clone)]
pub struct Time {
  /// The seconds
  pub secs: u64,
  /// The milliseconds
  pub millis: u32,
  /// The nano seconds
  pub nanos: u32,
}

impl Time {
  /// Create a new time object
  pub fn new(secs: u64, millis: u32, nanos: u32) -> Self {
    Time {
      secs: secs,
      millis: millis,
      nanos: nanos,
    }
  }
}

impl fmt::Debug for Time {
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    write!(f, "{}.{}", self.secs, self.millis)
  }
}
