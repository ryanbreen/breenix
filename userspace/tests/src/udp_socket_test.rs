//! UDP Socket userspace test (std version)
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

use libbreenix::error::Error;
use libbreenix::errno::Errno;
use libbreenix::io;
use libbreenix::socket::{self, SockAddrIn, AF_INET, SOCK_DGRAM, SOCK_NONBLOCK};
use std::process;

fn main() {
    println!("UDP Socket Test: Starting");

    // Step 1: Create UDP socket
    println!("UDP Socket Test: Creating socket...");
    let fd = match socket::socket(AF_INET, SOCK_DGRAM, 0) {
        Ok(fd) => fd,
        Err(e) => {
            println!("UDP Socket Test: FAILED to create socket, errno={:?}", e);
            process::exit(1);
        }
    };
    println!("UDP: Socket created fd={}", fd.raw());

    // Step 2: Bind to local port 12345
    println!("UDP Socket Test: Binding to port 12345...");
    let local_addr = SockAddrIn::new([0, 0, 0, 0], 12345);
    if let Err(e) = socket::bind_inet(fd, &local_addr) {
        println!("UDP Socket Test: FAILED to bind, errno={:?}", e);
        process::exit(2);
    }
    println!("UDP: Socket bound to port 12345");

    // Step 3: Send UDP packet to gateway (10.0.2.2 for SLIRP)
    println!("UDP Socket Test: Sending packet to gateway...");
    let gateway_addr = SockAddrIn::new([10, 0, 2, 2], 7777);
    let message = b"Hello from Breenix UDP!";

    match socket::sendto(fd, message, &gateway_addr) {
        Ok(bytes) => {
            println!("UDP: Packet sent successfully, bytes={}", bytes);
        }
        Err(e) => {
            println!("UDP Socket Test: FAILED to send, errno={:?}", e);
            process::exit(3);
        }
    }

    // Step 4: Create a second socket for RX testing
    println!("UDP Socket Test: Creating RX test socket...");
    let rx_fd = match socket::socket(AF_INET, SOCK_DGRAM, 0) {
        Ok(fd) => fd,
        Err(e) => {
            println!("UDP Socket Test: FAILED to create RX socket, errno={:?}", e);
            process::exit(4);
        }
    };
    println!("UDP: RX socket created fd={}", rx_fd.raw());

    // Bind to port 54321 for RX testing
    println!("UDP Socket Test: Binding RX socket to port 54321...");
    let rx_addr = SockAddrIn::new([0, 0, 0, 0], 54321);
    if let Err(e) = socket::bind_inet(rx_fd, &rx_addr) {
        println!("UDP Socket Test: FAILED to bind RX socket, errno={:?}", e);
        process::exit(5);
    }
    println!("UDP: RX socket bound to port 54321");

    // Step 5: Send a packet to ourselves to test RX
    println!("UDP Socket Test: Sending packet to ourselves (loopback test)...");
    let loopback_addr = SockAddrIn::new([10, 0, 2, 15], 54321);
    let test_message = b"RX TEST";

    match socket::sendto(fd, test_message, &loopback_addr) {
        Ok(bytes) => {
            println!("UDP: Loopback packet sent, bytes={}", bytes);
        }
        Err(e) => {
            println!("UDP Socket Test: FAILED to send loopback packet, errno={:?}", e);
            process::exit(6);
        }
    }

    // Step 6: Try to receive the packet
    println!("UDP Socket Test: Attempting to receive packet...");
    let mut recv_buf = [0u8; 128];
    let mut src_addr = SockAddrIn::default();

    match socket::recvfrom(rx_fd, &mut recv_buf, Some(&mut src_addr)) {
        Ok(bytes) => {
            println!("UDP: Received packet! bytes={}", bytes);

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
                    println!("UDP: RX data matches TX data - SUCCESS!");
                } else {
                    println!("UDP Socket Test: FAILED - RX data does not match TX data");
                    process::exit(7);
                }
            } else {
                println!("UDP Socket Test: FAILED - wrong packet size received");
                process::exit(8);
            }
        }
        Err(e) => {
            println!("UDP Socket Test: FAILED - recvfrom returned errno={:?}", e);
            println!("UDP Socket Test: Loopback packet was not received.");
            println!("UDP Socket Test: This indicates a bug in the loopback delivery path.");
            process::exit(9);
        }
    }

    // =========================================================================
    // Test 7: Ephemeral Port Test - bind(port=0) allocates a port
    // =========================================================================
    println!("UDP Ephemeral Port Test: Starting...");

    let ephemeral_fd = match socket::socket(AF_INET, SOCK_DGRAM, 0) {
        Ok(fd) => fd,
        Err(e) => {
            println!("UDP Ephemeral Port Test: FAILED to create socket, errno={:?}", e);
            process::exit(10);
        }
    };
    println!("UDP: Ephemeral socket created fd={}", ephemeral_fd.raw());

    // Bind to port 0 - kernel should allocate an ephemeral port
    let ephemeral_addr = SockAddrIn::new([0, 0, 0, 0], 0);
    if let Err(e) = socket::bind_inet(ephemeral_fd, &ephemeral_addr) {
        println!("UDP Ephemeral Port Test: FAILED to bind to port 0, errno={:?}", e);
        process::exit(11);
    }
    println!("UDP_EPHEMERAL_TEST: port 0 bind OK");

    // Close ephemeral socket
    let _ = io::close(ephemeral_fd);

    // =========================================================================
    // Test 8: EADDRINUSE Test - binding to already-bound port fails
    // =========================================================================
    println!("UDP EADDRINUSE Test: Starting...");

    let conflict_fd1 = match socket::socket(AF_INET, SOCK_DGRAM, 0) {
        Ok(fd) => fd,
        Err(e) => {
            println!("UDP EADDRINUSE Test: FAILED to create socket1, errno={:?}", e);
            process::exit(12);
        }
    };

    // Bind first socket to port 54324
    let conflict_addr = SockAddrIn::new([0, 0, 0, 0], 54324);
    if let Err(e) = socket::bind_inet(conflict_fd1, &conflict_addr) {
        println!("UDP EADDRINUSE Test: FAILED to bind first socket, errno={:?}", e);
        process::exit(13);
    }
    println!("UDP: First socket bound to port 54324");

    // Create second socket and try to bind to same port
    let conflict_fd2 = match socket::socket(AF_INET, SOCK_DGRAM, 0) {
        Ok(fd) => fd,
        Err(e) => {
            println!("UDP EADDRINUSE Test: FAILED to create socket2, errno={:?}", e);
            process::exit(14);
        }
    };

    match socket::bind_inet(conflict_fd2, &conflict_addr) {
        Ok(()) => {
            println!("UDP EADDRINUSE Test: FAILED - second bind should have failed!");
            process::exit(15);
        }
        Err(Error::Os(Errno::EADDRINUSE)) => {
            println!("UDP_EADDRINUSE_TEST: conflict detected OK");
        }
        Err(e) => {
            println!("UDP EADDRINUSE Test: FAILED - expected EADDRINUSE(98), got {:?}", e);
            process::exit(16);
        }
    }

    // Close conflict sockets
    let _ = io::close(conflict_fd1);
    let _ = io::close(conflict_fd2);

    // =========================================================================
    // Test 9: EAGAIN Test - recvfrom on empty queue returns EAGAIN
    // =========================================================================
    println!("UDP EAGAIN Test: Starting...");

    let eagain_fd = match socket::socket(AF_INET, SOCK_DGRAM | SOCK_NONBLOCK, 0) {
        Ok(fd) => fd,
        Err(e) => {
            println!("UDP EAGAIN Test: FAILED to create socket, errno={:?}", e);
            process::exit(17);
        }
    };

    // Bind to a port
    let eagain_addr = SockAddrIn::new([0, 0, 0, 0], 54325);
    if let Err(e) = socket::bind_inet(eagain_fd, &eagain_addr) {
        println!("UDP EAGAIN Test: FAILED to bind, errno={:?}", e);
        process::exit(18);
    }
    println!("UDP: EAGAIN test socket bound to port 54325");

    // Try to receive without any data sent - should return EAGAIN
    let mut eagain_buf = [0u8; 64];
    match socket::recvfrom(eagain_fd, &mut eagain_buf, None) {
        Ok(_) => {
            println!("UDP EAGAIN Test: FAILED - recvfrom should have returned EAGAIN!");
            process::exit(19);
        }
        Err(Error::Os(Errno::EAGAIN)) => {
            println!("UDP_EAGAIN_TEST: empty queue OK");
        }
        Err(e) => {
            println!("UDP EAGAIN Test: FAILED - expected EAGAIN(11), got {:?}", e);
            process::exit(20);
        }
    }

    // Close eagain socket
    let _ = io::close(eagain_fd);

    // =========================================================================
    // Test 10: Multiple Packets Test - multiple packets queued and received
    // =========================================================================
    println!("UDP Multiple Packets Test: Starting...");

    // Create receiver socket
    let multi_rx_fd = match socket::socket(AF_INET, SOCK_DGRAM, 0) {
        Ok(fd) => fd,
        Err(e) => {
            println!("UDP Multiple Packets Test: FAILED to create RX socket, errno={:?}", e);
            process::exit(21);
        }
    };

    // Bind receiver to port 54326
    let multi_rx_addr = SockAddrIn::new([0, 0, 0, 0], 54326);
    if let Err(e) = socket::bind_inet(multi_rx_fd, &multi_rx_addr) {
        println!("UDP Multiple Packets Test: FAILED to bind RX socket, errno={:?}", e);
        process::exit(22);
    }
    println!("UDP: Multi-packet RX socket bound to port 54326");

    // Create sender socket
    let multi_tx_fd = match socket::socket(AF_INET, SOCK_DGRAM, 0) {
        Ok(fd) => fd,
        Err(e) => {
            println!("UDP Multiple Packets Test: FAILED to create TX socket, errno={:?}", e);
            process::exit(23);
        }
    };

    // Bind sender to a different port
    let multi_tx_addr = SockAddrIn::new([0, 0, 0, 0], 54327);
    if let Err(e) = socket::bind_inet(multi_tx_fd, &multi_tx_addr) {
        println!("UDP Multiple Packets Test: FAILED to bind TX socket, errno={:?}", e);
        process::exit(24);
    }
    println!("UDP: Multi-packet TX socket bound to port 54327");

    // Destination address (loopback to receiver)
    let multi_dest = SockAddrIn::new([10, 0, 2, 15], 54326);

    // Send 3 different packets
    let packets: [&[u8]; 3] = [b"PKT1", b"PKT2", b"PKT3"];
    for (i, pkt) in packets.iter().enumerate() {
        match socket::sendto(multi_tx_fd, pkt, &multi_dest) {
            Ok(bytes) => {
                println!("UDP: Sent packet {}, bytes={}", i + 1, bytes);
            }
            Err(e) => {
                println!("UDP Multiple Packets Test: FAILED to send packet {}, errno={:?}", i + 1, e);
                process::exit(25);
            }
        }
    }

    // Receive all 3 packets and verify data
    let mut multi_recv_buf = [0u8; 64];
    let mut received_count = 0;

    for i in 0..3 {
        match socket::recvfrom(multi_rx_fd, &mut multi_recv_buf, None) {
            Ok(bytes) => {
                println!("UDP: Received packet {}, bytes={}", i + 1, bytes);

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
                        println!("UDP Multiple Packets Test: Data mismatch for packet {}", i + 1);
                    }
                } else {
                    println!("UDP Multiple Packets Test: Wrong size for packet {}", i + 1);
                }
            }
            Err(e) => {
                println!("UDP Multiple Packets Test: FAILED to receive packet {}, errno={:?}", i + 1, e);
                continue;
            }
        }
    }

    if received_count == 3 {
        println!("UDP_MULTIPACKET_TEST: 3 packets OK");
    } else {
        println!("UDP Multiple Packets Test: FAILED - only received {} of 3 packets", received_count);
        process::exit(26);
    }

    // Close multi-packet sockets
    let _ = io::close(multi_rx_fd);
    let _ = io::close(multi_tx_fd);

    // Close the original test sockets
    let _ = io::close(fd);
    let _ = io::close(rx_fd);

    println!("UDP Socket Test: All tests passed!");
    process::exit(0);
}
