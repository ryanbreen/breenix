//! Hello World using breenix_std
//!
//! This test demonstrates Stage 1 of Breenix std support:
//! - println! macro for formatted output
//! - Heap allocation (Vec, String, Box)
//! - Panic handler with location info
//!
//! This is the first userspace program to use std-like APIs!

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use breenix_std::prelude::*;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Test basic println!
    println!("=== Breenix std Stage 1 Test ===");
    println!();

    // Test formatted output
    println!("Hello from {}!", "breenix_std");
    println!("The answer is: {}", 42);

    // Test heap allocation with Vec
    println!("\n--- Testing Vec ---");
    let mut numbers: Vec<i32> = vec![1, 2, 3, 4, 5];
    println!("Created vector with {} elements", numbers.len());

    numbers.push(6);
    println!("After push: {} elements", numbers.len());

    // Calculate sum
    let sum: i32 = numbers.iter().sum();
    println!("Sum of elements: {}", sum);

    // Test String
    println!("\n--- Testing String ---");
    let mut greeting = String::from("Hello");
    greeting.push_str(", Breenix!");
    println!("String: {}", greeting);
    println!("String length: {} bytes", greeting.len());

    // Test Box
    println!("\n--- Testing Box ---");
    let boxed_value = Box::new(12345u64);
    println!("Boxed value: {}", *boxed_value);

    // Test format! macro
    println!("\n--- Testing format! ---");
    let formatted = format!("x={}, y={}, z={}", 10, 20, 30);
    println!("Formatted string: {}", formatted);

    // Success message
    println!();
    println!("=== All Stage 1 tests passed! ===");
    println!("HELLO_STD_COMPLETE");

    exit(0);
}
