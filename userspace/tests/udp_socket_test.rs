//! UDP Socket userspace test
//!
//! Tests the UDP socket syscalls from userspace:
//! 1. Create a UDP socket
//! 2. Bind to a local port
//! 3. Send a UDP packet to the gateway (TX test)
//! 4. Receive a UDP packet (RX test via loopback)
//! 5. Verify RX data matches TX data
//! 6. Ephemeral port test - bind(port=0)
//! 7. EADDRINUSE test - bind to already-bound port
//! 8. EAGAIN test - recvfrom on empty queue
//! 9. Multiple packets test - queue and receive 3 packets
//!
//! This validates the full userspace -> kernel -> network stack path.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::socket::{socket, bind, sendto, recvfrom, SockAddrIn, AF_INET, SOCK_DGRAM, SOCK_NONBLOCK};

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

    // =========================================================================
    // Test 7: Ephemeral Port Test - bind(port=0) allocates a port
    // =========================================================================
    io::print("UDP Ephemeral Port Test: Starting...\n");

    let ephemeral_fd = match socket(AF_INET, SOCK_DGRAM, 0) {
        Ok(fd) => {
            io::print("UDP: Ephemeral socket created fd=");
            print_num(fd as u64);
            io::print("\n");
            fd
        }
        Err(e) => {
            io::print("UDP Ephemeral Port Test: FAILED to create socket, errno=");
            print_num(e as u64);
            io::print("\n");
            process::exit(10);
        }
    };

    // Bind to port 0 - kernel should allocate an ephemeral port
    let ephemeral_addr = SockAddrIn::new([0, 0, 0, 0], 0);
    match bind(ephemeral_fd, &ephemeral_addr) {
        Ok(()) => {
            io::print("UDP_EPHEMERAL_TEST: port 0 bind OK\n");
        }
        Err(e) => {
            io::print("UDP Ephemeral Port Test: FAILED to bind to port 0, errno=");
            print_num(e as u64);
            io::print("\n");
            process::exit(11);
        }
    }

    // Close ephemeral socket
    io::close(ephemeral_fd as u64);

    // =========================================================================
    // Test 8: EADDRINUSE Test - binding to already-bound port fails
    // =========================================================================
    io::print("UDP EADDRINUSE Test: Starting...\n");

    // EADDRINUSE = 98 (Linux)
    const EADDRINUSE: i32 = 98;

    let conflict_fd1 = match socket(AF_INET, SOCK_DGRAM, 0) {
        Ok(fd) => fd,
        Err(e) => {
            io::print("UDP EADDRINUSE Test: FAILED to create socket1, errno=");
            print_num(e as u64);
            io::print("\n");
            process::exit(12);
        }
    };

    // Bind first socket to port 54321
    let conflict_addr = SockAddrIn::new([0, 0, 0, 0], 54324);
    match bind(conflict_fd1, &conflict_addr) {
        Ok(()) => {
            io::print("UDP: First socket bound to port 54324\n");
        }
        Err(e) => {
            io::print("UDP EADDRINUSE Test: FAILED to bind first socket, errno=");
            print_num(e as u64);
            io::print("\n");
            process::exit(13);
        }
    }

    // Create second socket and try to bind to same port
    let conflict_fd2 = match socket(AF_INET, SOCK_DGRAM, 0) {
        Ok(fd) => fd,
        Err(e) => {
            io::print("UDP EADDRINUSE Test: FAILED to create socket2, errno=");
            print_num(e as u64);
            io::print("\n");
            process::exit(14);
        }
    };

    match bind(conflict_fd2, &conflict_addr) {
        Ok(()) => {
            io::print("UDP EADDRINUSE Test: FAILED - second bind should have failed!\n");
            process::exit(15);
        }
        Err(EADDRINUSE) => {
            io::print("UDP_EADDRINUSE_TEST: conflict detected OK\n");
        }
        Err(e) => {
            io::print("UDP EADDRINUSE Test: FAILED - expected EADDRINUSE(98), got errno=");
            print_num(e as u64);
            io::print("\n");
            process::exit(16);
        }
    }

    // Close conflict sockets
    io::close(conflict_fd1 as u64);
    io::close(conflict_fd2 as u64);

    // =========================================================================
    // Test 9: EAGAIN Test - recvfrom on empty queue returns EAGAIN
    // =========================================================================
    io::print("UDP EAGAIN Test: Starting...\n");

    // EAGAIN = 11 (Linux)
    const EAGAIN: i32 = 11;

    let eagain_fd = match socket(AF_INET, SOCK_DGRAM | SOCK_NONBLOCK, 0) {
        Ok(fd) => fd,
        Err(e) => {
            io::print("UDP EAGAIN Test: FAILED to create socket, errno=");
            print_num(e as u64);
            io::print("\n");
            process::exit(17);
        }
    };

    // Bind to a port
    let eagain_addr = SockAddrIn::new([0, 0, 0, 0], 54325);
    match bind(eagain_fd, &eagain_addr) {
        Ok(()) => {
            io::print("UDP: EAGAIN test socket bound to port 54325\n");
        }
        Err(e) => {
            io::print("UDP EAGAIN Test: FAILED to bind, errno=");
            print_num(e as u64);
            io::print("\n");
            process::exit(18);
        }
    }

    // Try to receive without any data sent - should return EAGAIN
    let mut eagain_buf = [0u8; 64];
    let mut eagain_src = SockAddrIn::new([0, 0, 0, 0], 0);
    match recvfrom(eagain_fd, &mut eagain_buf, Some(&mut eagain_src)) {
        Ok(_) => {
            io::print("UDP EAGAIN Test: FAILED - recvfrom should have returned EAGAIN!\n");
            process::exit(19);
        }
        Err(EAGAIN) => {
            io::print("UDP_EAGAIN_TEST: empty queue OK\n");
        }
        Err(e) => {
            io::print("UDP EAGAIN Test: FAILED - expected EAGAIN(11), got errno=");
            print_num(e as u64);
            io::print("\n");
            process::exit(20);
        }
    }

    // Close eagain socket
    io::close(eagain_fd as u64);

    // =========================================================================
    // Test 10: Multiple Packets Test - multiple packets queued and received
    // =========================================================================
    io::print("UDP Multiple Packets Test: Starting...\n");

    // Create receiver socket
    let multi_rx_fd = match socket(AF_INET, SOCK_DGRAM, 0) {
        Ok(fd) => fd,
        Err(e) => {
            io::print("UDP Multiple Packets Test: FAILED to create RX socket, errno=");
            print_num(e as u64);
            io::print("\n");
            process::exit(21);
        }
    };

    // Bind receiver to port 54326
    let multi_rx_addr = SockAddrIn::new([0, 0, 0, 0], 54326);
    match bind(multi_rx_fd, &multi_rx_addr) {
        Ok(()) => {
            io::print("UDP: Multi-packet RX socket bound to port 54326\n");
        }
        Err(e) => {
            io::print("UDP Multiple Packets Test: FAILED to bind RX socket, errno=");
            print_num(e as u64);
            io::print("\n");
            process::exit(22);
        }
    }

    // Create sender socket
    let multi_tx_fd = match socket(AF_INET, SOCK_DGRAM, 0) {
        Ok(fd) => fd,
        Err(e) => {
            io::print("UDP Multiple Packets Test: FAILED to create TX socket, errno=");
            print_num(e as u64);
            io::print("\n");
            process::exit(23);
        }
    };

    // Bind sender to a different port
    let multi_tx_addr = SockAddrIn::new([0, 0, 0, 0], 54327);
    match bind(multi_tx_fd, &multi_tx_addr) {
        Ok(()) => {
            io::print("UDP: Multi-packet TX socket bound to port 54327\n");
        }
        Err(e) => {
            io::print("UDP Multiple Packets Test: FAILED to bind TX socket, errno=");
            print_num(e as u64);
            io::print("\n");
            process::exit(24);
        }
    }

    // Destination address (loopback to receiver)
    let multi_dest = SockAddrIn::new([10, 0, 2, 15], 54326);

    // Send 3 different packets
    let packets: [&[u8]; 3] = [b"PKT1", b"PKT2", b"PKT3"];
    for (i, pkt) in packets.iter().enumerate() {
        match sendto(multi_tx_fd, pkt, &multi_dest) {
            Ok(bytes) => {
                io::print("UDP: Sent packet ");
                print_num((i + 1) as u64);
                io::print(", bytes=");
                print_num(bytes as u64);
                io::print("\n");
            }
            Err(e) => {
                io::print("UDP Multiple Packets Test: FAILED to send packet ");
                print_num((i + 1) as u64);
                io::print(", errno=");
                print_num(e as u64);
                io::print("\n");
                process::exit(25);
            }
        }
    }

    // Receive all 3 packets and verify data
    let mut multi_recv_buf = [0u8; 64];
    let mut multi_recv_src = SockAddrIn::new([0, 0, 0, 0], 0);
    let mut received_count = 0;

    for i in 0..3 {
        match recvfrom(multi_rx_fd, &mut multi_recv_buf, Some(&mut multi_recv_src)) {
            Ok(bytes) => {
                io::print("UDP: Received packet ");
                print_num((i + 1) as u64);
                io::print(", bytes=");
                print_num(bytes as u64);
                io::print("\n");

                // Verify data matches expected packet
                let expected = packets[i];
                if bytes == expected.len() {
                    let mut matches = true;
                    for j in 0..bytes {
                        if multi_recv_buf[j] != expected[j] {
                            matches = false;
                            break;
                        }
                    }
                    if matches {
                        received_count += 1;
                    } else {
                        io::print("UDP Multiple Packets Test: Data mismatch for packet ");
                        print_num((i + 1) as u64);
                        io::print("\n");
                    }
                } else {
                    io::print("UDP Multiple Packets Test: Wrong size for packet ");
                    print_num((i + 1) as u64);
                    io::print("\n");
                }
            }
            Err(e) => {
                io::print("UDP Multiple Packets Test: FAILED to receive packet ");
                print_num((i + 1) as u64);
                io::print(", errno=");
                print_num(e as u64);
                io::print("\n");
                // Continue trying to receive remaining packets
            }
        }
    }

    if received_count == 3 {
        io::print("UDP_MULTIPACKET_TEST: 3 packets OK\n");
    } else {
        io::print("UDP Multiple Packets Test: FAILED - only received ");
        print_num(received_count as u64);
        io::print(" of 3 packets\n");
        process::exit(26);
    }

    // Close multi-packet sockets
    io::close(multi_rx_fd as u64);
    io::close(multi_tx_fd as u64);

    // Close the original test sockets
    io::close(fd as u64);
    io::close(rx_fd as u64);

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
