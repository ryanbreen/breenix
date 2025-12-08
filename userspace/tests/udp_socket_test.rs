//! UDP Socket userspace test
//!
//! Tests the UDP socket syscalls from userspace:
//! 1. Create a UDP socket
//! 2. Bind to a local port
//! 3. Send a UDP packet to the gateway (TX test)
//! 4. Receive a UDP packet (RX test)
//!
//! This validates the full userspace -> kernel -> network stack path.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::socket::{socket, bind, sendto, recvfrom, SockAddrIn, AF_INET, SOCK_DGRAM};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    io::print("UDP Socket Test: Starting\n");

    // Step 1: Create UDP socket
    io::print("UDP Socket Test: Creating socket...\n");
    let fd = match socket(AF_INET, SOCK_DGRAM, 0) {
        Ok(fd) => {
            io::print("UDP: Socket created fd=");
            print_num(fd as u64);
            io::print("\n");
            fd
        }
        Err(e) => {
            io::print("UDP Socket Test: FAILED to create socket, errno=");
            print_num(e as u64);
            io::print("\n");
            process::exit(1);
        }
    };

    // Step 2: Bind to local port 12345
    io::print("UDP Socket Test: Binding to port 12345...\n");
    let local_addr = SockAddrIn::new([0, 0, 0, 0], 12345);
    match bind(fd, &local_addr) {
        Ok(()) => {
            io::print("UDP: Socket bound to port 12345\n");
        }
        Err(e) => {
            io::print("UDP Socket Test: FAILED to bind, errno=");
            print_num(e as u64);
            io::print("\n");
            process::exit(2);
        }
    }

    // Step 3: Send UDP packet to gateway (10.0.2.2 for SLIRP, 192.168.105.1 for vmnet)
    // Using SLIRP gateway as default
    io::print("UDP Socket Test: Sending packet to gateway...\n");
    let gateway_addr = SockAddrIn::new([10, 0, 2, 2], 7777); // Echo port or any port
    let message = b"Hello from Breenix UDP!";

    match sendto(fd, message, &gateway_addr) {
        Ok(bytes) => {
            io::print("UDP: Packet sent successfully, bytes=");
            print_num(bytes as u64);
            io::print("\n");
        }
        Err(e) => {
            io::print("UDP Socket Test: FAILED to send, errno=");
            print_num(e as u64);
            io::print("\n");
            process::exit(3);
        }
    }

    // Step 4: Create a second socket for RX testing
    io::print("UDP Socket Test: Creating RX test socket...\n");
    let rx_fd = match socket(AF_INET, SOCK_DGRAM, 0) {
        Ok(fd) => {
            io::print("UDP: RX socket created fd=");
            print_num(fd as u64);
            io::print("\n");
            fd
        }
        Err(e) => {
            io::print("UDP Socket Test: FAILED to create RX socket, errno=");
            print_num(e as u64);
            io::print("\n");
            process::exit(4);
        }
    };

    // Bind to port 54321 for RX testing
    io::print("UDP Socket Test: Binding RX socket to port 54321...\n");
    let rx_addr = SockAddrIn::new([0, 0, 0, 0], 54321);
    match bind(rx_fd, &rx_addr) {
        Ok(()) => {
            io::print("UDP: RX socket bound to port 54321\n");
        }
        Err(e) => {
            io::print("UDP Socket Test: FAILED to bind RX socket, errno=");
            print_num(e as u64);
            io::print("\n");
            process::exit(5);
        }
    }

    // Step 5: Send a packet to ourselves to test RX
    io::print("UDP Socket Test: Sending packet to ourselves (loopback test)...\n");
    let loopback_addr = SockAddrIn::new([10, 0, 2, 15], 54321); // Our own IP:port
    let test_message = b"RX TEST";

    match sendto(fd, test_message, &loopback_addr) {
        Ok(bytes) => {
            io::print("UDP: Loopback packet sent, bytes=");
            print_num(bytes as u64);
            io::print("\n");
        }
        Err(e) => {
            io::print("UDP Socket Test: FAILED to send loopback packet, errno=");
            print_num(e as u64);
            io::print("\n");
            process::exit(6);
        }
    }

    // Step 6: Try to receive the packet
    io::print("UDP Socket Test: Attempting to receive packet...\n");
    let mut recv_buf = [0u8; 128];
    let mut src_addr = SockAddrIn::new([0, 0, 0, 0], 0);

    match recvfrom(rx_fd, &mut recv_buf, Some(&mut src_addr)) {
        Ok(bytes) => {
            io::print("UDP: Received packet! bytes=");
            print_num(bytes as u64);
            io::print("\n");

            // Verify the data matches what we sent
            if bytes == test_message.len() {
                let mut matches = true;
                for i in 0..bytes {
                    if recv_buf[i] != test_message[i] {
                        matches = false;
                        break;
                    }
                }
                if matches {
                    io::print("UDP: RX data matches TX data - SUCCESS!\n");
                } else {
                    io::print("UDP Socket Test: FAILED - RX data does not match TX data\n");
                    process::exit(7);
                }
            } else {
                io::print("UDP Socket Test: FAILED - wrong packet size received\n");
                process::exit(8);
            }
        }
        Err(e) => {
            // Loopback RX failed - this is a real test failure
            io::print("UDP Socket Test: FAILED - recvfrom returned errno=");
            print_num(e as u64);
            io::print("\n");
            io::print("UDP Socket Test: Loopback packet was not received.\n");
            io::print("UDP Socket Test: This indicates a bug in the loopback delivery path.\n");
            process::exit(9);
        }
    }

    io::print("UDP Socket Test: All tests passed!\n");
    process::exit(0);
}

/// Simple number printing (no formatting)
fn print_num(mut n: u64) {
    if n == 0 {
        io::print("0");
        return;
    }

    let mut buf = [0u8; 20];
    let mut i = 0;

    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }

    // Reverse and print
    while i > 0 {
        i -= 1;
        let ch = [buf[i]];
        // Safe because we only put ASCII digits
        if let Ok(s) = core::str::from_utf8(&ch) {
            io::print(s);
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("UDP Socket Test: PANIC!\n");
    process::exit(99);
}
