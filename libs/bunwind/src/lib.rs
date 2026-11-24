#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]

pub mod cfi;

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod bunwind;
