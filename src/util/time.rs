use core::cmp::Ordering;
use core::ops::{Add, Sub};

use constants::timer::{NANOS_PER_MICRO, NANOS_PER_MILLI, NANOS_PER_SEC};

/// A duration
#[derive(Copy, Clone)]
pub struct Duration {
  /// The seconds
  pub secs: i64,
  /// The nano seconds
  pub nanos: i32,
}

impl Duration {
  /// Create a new duration
  pub fn new(mut secs: i64, mut nanos: i32) -> Self {
    // TODO: This is weird.  Why not just math?
    while nanos >= NANOS_PER_SEC || (nanos > 0 && secs < 0) {
      secs += 1;
      nanos -= NANOS_PER_SEC;
    }

    // TODO: This is weird.  Why not just math?
    while nanos < 0 && secs > 0 {
      secs -= 1;
      nanos += NANOS_PER_SEC;
    }

    Duration {
      secs: secs,
      nanos: nanos,
    }
  }
}

impl Add for Duration {
  type Output = Duration;

  fn add(self, other: Self) -> Self {
    Duration::new(self.secs + other.secs, self.nanos + other.nanos)
  }
}

impl Sub for Duration {
  type Output = Duration;

  fn sub(self, other: Self) -> Self {
    Duration::new(self.secs - other.secs, self.nanos - other.nanos)
  }
}

impl PartialEq for Duration {
  fn eq(&self, other: &Self) -> bool {
    let dif = *self - *other;
    dif.secs == 0 && dif.nanos == 0
  }
}

impl PartialOrd for Duration {
  fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
    let dif = *self - *other;
    if dif.secs > 0 {
      Some(Ordering::Greater)
    } else if dif.secs < 0 {
      Some(Ordering::Less)
    } else if dif.nanos > 0 {
      Some(Ordering::Greater)
    } else if dif.nanos < 0 {
      Some(Ordering::Less)
    } else {
      Some(Ordering::Equal)
    }
  }
}
