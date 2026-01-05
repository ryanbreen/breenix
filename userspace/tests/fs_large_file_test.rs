//! Large file test for ext2 filesystem (double indirect blocks)
//!
//! Tests writing and reading files larger than what single indirect blocks can handle,
//! which requires double indirect block support in the ext2 implementation.
//!
//! # Block Addressing Coverage
//!
//! With 1KB blocks (used in our 4MB test ext2.img):
//! - Direct: 12 blocks = 12KB
//! - Single indirect: 256 blocks = 256KB (268KB total)
//! - Double indirect: 256*256 = 65,536 blocks = 64MB (64MB + 268KB total)
//! - Triple indirect: 256^3 = 16,777,216 blocks = 16GB (would require > 64MB file)
//!
//! # Test Coverage
//!
//! This test writes 512KB which exercises double indirect blocks with 1KB block size
//! while fitting within the 4MB test filesystem.
//!
//! # Limitation: Triple Indirect Blocks Untested
//!
//! Triple indirect block support exists in the implementation (see
//! `kernel/src/fs/ext2/file.rs` - both `get_block_num` and `set_block_num` have
//! complete triple indirect logic), but **cannot be tested** with the current 4MB
//! test filesystem. Testing triple indirect blocks would require:
//!
//! - A filesystem image > 64MB (to create files that exceed double indirect capacity)
//! - For 1KB blocks: file size > 64MB + 268KB to enter triple indirect territory
//! - For 4KB blocks: file size > 4GB + ~4MB to need triple indirect blocks
//!
//! The triple indirect code paths are therefore untested in the current test suite.
//! If larger filesystem testing becomes necessary, consider creating a larger test
//! image or using a loopback device

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::fs::{
    close, lseek, open, open_with_mode, read, unlink, write,
    O_CREAT, O_RDONLY, O_TRUNC, O_WRONLY, SEEK_SET,
};
use libbreenix::io::println;
use libbreenix::process::exit;

/// Helper to print a number (simple decimal conversion)
fn print_num(n: usize) {
    if n == 0 {
        libbreenix::io::print("0");
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = 0;
    let mut num = n;
    while num > 0 {
        buf[i] = b'0' + (num % 10) as u8;
        num /= 10;
        i += 1;
    }
    // Reverse and print
    while i > 0 {
        i -= 1;
        libbreenix::io::print(unsafe { core::str::from_utf8_unchecked(&buf[i..i + 1]) });
    }
}

/// Generate a test pattern for a given position in the file
/// This allows verification that we read back exactly what we wrote
fn generate_pattern_byte(position: usize) -> u8 {
    // Use a simple but recognizable pattern that varies by position
    // (position % 251) gives 0-250, then we add the high nibble of position
    // This ensures different blocks have different patterns
    let base = (position % 251) as u8;
    let block_marker = ((position / 1024) % 256) as u8;
    base.wrapping_add(block_marker)
}

/// Verify that a buffer matches the expected pattern at the given offset
fn verify_pattern(buf: &[u8], offset: usize) -> bool {
    for (i, &byte) in buf.iter().enumerate() {
        let expected = generate_pattern_byte(offset + i);
        if byte != expected {
            libbreenix::io::print("Pattern mismatch at position ");
            print_num(offset + i);
            libbreenix::io::print(": expected ");
            print_num(expected as usize);
            libbreenix::io::print(" got ");
            print_num(byte as usize);
            libbreenix::io::print("\n");
            return false;
        }
    }
    true
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("Large file test (double indirect blocks) starting...");

    // We need to write a file larger than what single indirect blocks can handle.
    // With 1KB blocks (used in our test ext2.img), single indirect covers 268KB.
    // Writing 512KB exercises double indirect blocks while fitting in 4MB filesystem.
    const FILE_SIZE: usize = 512 * 1024; // 512KB - requires double indirect blocks with 1KB blocks
    const WRITE_CHUNK: usize = 1024; // Write 1KB at a time (matches our block size)

    // ============================================
    // Test 1: Write large file with pattern data
    // ============================================
    libbreenix::io::print("\nTest 1: Writing ");
    print_num(FILE_SIZE / 1024);
    libbreenix::io::print("KB - requires double indirect blocks\n");

    let fd = match open_with_mode("/large_test.bin\0", O_WRONLY | O_CREAT | O_TRUNC, 0o644) {
        Ok(fd) => fd,
        Err(_) => {
            println("FAILED: Could not create /large_test.bin");
            exit(1);
        }
    };

    // Write the file in chunks, using a pattern that varies by position
    let mut total_written = 0usize;
    let mut chunk_buf = [0u8; WRITE_CHUNK];

    while total_written < FILE_SIZE {
        let remaining = FILE_SIZE - total_written;
        let to_write = if remaining < WRITE_CHUNK { remaining } else { WRITE_CHUNK };

        // Fill chunk with pattern data
        for i in 0..to_write {
            chunk_buf[i] = generate_pattern_byte(total_written + i);
        }

        match write(fd, &chunk_buf[..to_write]) {
            Ok(n) => {
                if n != to_write {
                    libbreenix::io::print("FAILED: Short write at offset ");
                    print_num(total_written);
                    libbreenix::io::print(": wrote ");
                    print_num(n);
                    libbreenix::io::print(" expected ");
                    print_num(to_write);
                    libbreenix::io::print("\n");
                    let _ = close(fd);
                    exit(1);
                }
                total_written += n;
            }
            Err(_) => {
                libbreenix::io::print("FAILED: Write error at offset ");
                print_num(total_written);
                libbreenix::io::print("\n");
                let _ = close(fd);
                exit(1);
            }
        }

        // Progress indicator every 128KB
        if total_written % (128 * 1024) == 0 && total_written > 0 {
            libbreenix::io::print("  Written ");
            print_num(total_written / 1024);
            libbreenix::io::print("KB...\n");
        }
    }

    let _ = close(fd);
    libbreenix::io::print("  Total written: ");
    print_num(total_written);
    libbreenix::io::print(" bytes\n");

    // ============================================
    // Test 2: Read back and verify all data
    // ============================================
    libbreenix::io::print("\nTest 2: Reading and verifying all ");
    print_num(FILE_SIZE);
    libbreenix::io::print(" bytes\n");

    let fd = match open("/large_test.bin\0", O_RDONLY) {
        Ok(fd) => fd,
        Err(_) => {
            println("FAILED: Could not reopen /large_test.bin for reading");
            exit(1);
        }
    };

    // Read and verify in chunks
    let mut total_read = 0usize;
    let mut read_buf = [0u8; WRITE_CHUNK];

    while total_read < FILE_SIZE {
        let remaining = FILE_SIZE - total_read;
        let to_read = if remaining < WRITE_CHUNK { remaining } else { WRITE_CHUNK };

        match read(fd, &mut read_buf[..to_read]) {
            Ok(n) => {
                if n == 0 {
                    libbreenix::io::print("FAILED: Unexpected EOF at offset ");
                    print_num(total_read);
                    libbreenix::io::print("\n");
                    let _ = close(fd);
                    exit(1);
                }

                // Verify the pattern
                if !verify_pattern(&read_buf[..n], total_read) {
                    libbreenix::io::print("FAILED: Data corruption in block starting at ");
                    print_num(total_read);
                    libbreenix::io::print("\n");
                    let _ = close(fd);
                    exit(1);
                }

                total_read += n;
            }
            Err(_) => {
                libbreenix::io::print("FAILED: Read error at offset ");
                print_num(total_read);
                libbreenix::io::print("\n");
                let _ = close(fd);
                exit(1);
            }
        }

        // Progress indicator every 128KB
        if total_read % (128 * 1024) == 0 && total_read > 0 {
            libbreenix::io::print("  Verified ");
            print_num(total_read / 1024);
            libbreenix::io::print("KB...\n");
        }
    }

    let _ = close(fd);
    libbreenix::io::print("  Total verified: ");
    print_num(total_read);
    libbreenix::io::print(" bytes\n");

    // ============================================
    // Test 3: Random seek + read verification
    // ============================================
    libbreenix::io::print("\nTest 3: Random seek verification (testing double indirect block reads)\n");

    let fd = match open("/large_test.bin\0", O_RDONLY) {
        Ok(fd) => fd,
        Err(_) => {
            println("FAILED: Could not reopen for random read test");
            exit(1);
        }
    };

    // Test positions: direct, single indirect, and double indirect blocks
    // With 1KB blocks: direct=0-12KB, single indirect=12KB-268KB, double indirect=268KB+
    // File size is 512KB, so we test positions within that range
    let test_positions: [usize; 6] = [
        0,                   // Start of file (direct block 0)
        8 * 1024,            // 8KB - mid direct blocks (block 8 of 12)
        48 * 1024,           // 48KB - deep in single indirect area
        100 * 1024,          // 100KB - still in single indirect
        300 * 1024,          // 300KB - double indirect area (starts at 268KB)
        512 * 1024 - 128,    // Near end of 512KB file
    ];

    for &pos in &test_positions {
        libbreenix::io::print("  Seeking to ");
        print_num(pos);
        libbreenix::io::print("...");

        match lseek(fd, pos as i64, SEEK_SET) {
            Ok(new_pos) => {
                if new_pos as usize != pos {
                    libbreenix::io::print(" FAILED: lseek returned wrong position\n");
                    let _ = close(fd);
                    exit(1);
                }
            }
            Err(_) => {
                libbreenix::io::print(" FAILED: lseek error\n");
                let _ = close(fd);
                exit(1);
            }
        }

        // Read a small chunk
        let mut small_buf = [0u8; 128];
        match read(fd, &mut small_buf) {
            Ok(n) => {
                if n == 0 {
                    libbreenix::io::print(" FAILED: EOF at position\n");
                    let _ = close(fd);
                    exit(1);
                }

                if !verify_pattern(&small_buf[..n], pos) {
                    libbreenix::io::print(" FAILED: Pattern mismatch\n");
                    let _ = close(fd);
                    exit(1);
                }

                libbreenix::io::print(" OK (");
                print_num(n);
                libbreenix::io::print(" bytes verified)\n");
            }
            Err(_) => {
                libbreenix::io::print(" FAILED: Read error\n");
                let _ = close(fd);
                exit(1);
            }
        }
    }

    let _ = close(fd);

    // ============================================
    // Cleanup
    // ============================================
    libbreenix::io::print("\nCleaning up /large_test.bin...\n");
    let _ = unlink("/large_test.bin\0");

    println("\nAll large file tests passed!");
    println("FS_LARGE_FILE_TEST_PASSED");
    exit(0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    libbreenix::io::print("PANIC in fs_large_file_test!\n");
    exit(2);
}
