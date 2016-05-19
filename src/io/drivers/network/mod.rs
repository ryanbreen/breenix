use core::fmt;

pub mod e1000;

use collections::String;

/*
 * Boosted from redox
 */

#[derive(Copy, Clone)]
pub struct MacAddr {
    pub bytes: [u8; 6],
}

impl MacAddr {
    pub fn equals(&self, other: Self) -> bool {
        for i in 0..6 {
            if self.bytes[i] != other.bytes[i] {
                return false;
            }
        }
        true
    }

    pub fn to_string(&self) -> String {
      format!("{:02x}::{:02x}::{:02x}::{:02x}::{:02x}::{:02x}",
        self.bytes[0],
        self.bytes[1],
        self.bytes[2],
        self.bytes[3],
        self.bytes[4],
        self.bytes[5])
    }
}

impl fmt::Display for MacAddr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.to_string())
    }
}