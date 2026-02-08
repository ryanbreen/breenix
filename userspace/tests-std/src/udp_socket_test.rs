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

use std::process;

#[repr(C)]
struct SockAddrIn {
    sin_family: u16,
    sin_port: u16,
    sin_addr: [u8; 4],
    sin_zero: [u8; 8],
}

impl SockAddrIn {
    fn new(addr: [u8; 4], port: u16) -> Self {
        SockAddrIn {
            sin_family: AF_INET as u16,
            sin_port: port.to_be(),
            sin_addr: addr,
            sin_zero: [0; 8],
        }
    }
}

const AF_INET: i32 = 2;
const SOCK_DGRAM: i32 = 2;
const SOCK_NONBLOCK: i32 = 0x800;

extern "C" {
    fn socket(domain: i32, typ: i32, protocol: i32) -> i32;
    fn bind(fd: i32, addr: *const SockAddrIn, addrlen: u32) -> i32;
    fn sendto(fd: i32, buf: *const u8, len: usize, flags: i32,
              dest_addr: *const SockAddrIn, addrlen: u32) -> isize;
    fn recvfrom(fd: i32, buf: *mut u8, len: usize, flags: i32,
                src_addr: *mut SockAddrIn, addrlen: *mut u32) -> isize;
    fn close(fd: i32) -> i32;
}

fn main() {
    println!("UDP Socket Test: Starting");

    // Step 1: Create UDP socket
    println!("UDP Socket Test: Creating socket...");
    let fd = unsafe { socket(AF_INET, SOCK_DGRAM, 0) };
    if fd < 0 {
        println!("UDP Socket Test: FAILED to create socket, errno={}", -fd);
        process::exit(1);
    }
    println!("UDP: Socket created fd={}", fd);

    // Step 2: Bind to local port 12345
    println!("UDP Socket Test: Binding to port 12345...");
    let local_addr = SockAddrIn::new([0, 0, 0, 0], 12345);
    let ret = unsafe { bind(fd, &local_addr, std::mem::size_of::<SockAddrIn>() as u32) };
    if ret < 0 {
        println!("UDP Socket Test: FAILED to bind, errno={}", -ret);
        process::exit(2);
    }
    println!("UDP: Socket bound to port 12345");

    // Step 3: Send UDP packet to gateway (10.0.2.2 for SLIRP)
    println!("UDP Socket Test: Sending packet to gateway...");
    let gateway_addr = SockAddrIn::new([10, 0, 2, 2], 7777);
    let message = b"Hello from Breenix UDP!";

    let bytes = unsafe {
        sendto(fd, message.as_ptr(), message.len(), 0,
               &gateway_addr, std::mem::size_of::<SockAddrIn>() as u32)
    };
    if bytes < 0 {
        println!("UDP Socket Test: FAILED to send, errno={}", -bytes);
        process::exit(3);
    }
    println!("UDP: Packet sent successfully, bytes={}", bytes);

    // Step 4: Create a second socket for RX testing
    println!("UDP Socket Test: Creating RX test socket...");
    let rx_fd = unsafe { socket(AF_INET, SOCK_DGRAM, 0) };
    if rx_fd < 0 {
        println!("UDP Socket Test: FAILED to create RX socket, errno={}", -rx_fd);
        process::exit(4);
    }
    println!("UDP: RX socket created fd={}", rx_fd);

    // Bind to port 54321 for RX testing
    println!("UDP Socket Test: Binding RX socket to port 54321...");
    let rx_addr = SockAddrIn::new([0, 0, 0, 0], 54321);
    let ret = unsafe { bind(rx_fd, &rx_addr, std::mem::size_of::<SockAddrIn>() as u32) };
    if ret < 0 {
        println!("UDP Socket Test: FAILED to bind RX socket, errno={}", -ret);
        process::exit(5);
    }
    println!("UDP: RX socket bound to port 54321");

    // Step 5: Send a packet to ourselves to test RX
    println!("UDP Socket Test: Sending packet to ourselves (loopback test)...");
    let loopback_addr = SockAddrIn::new([10, 0, 2, 15], 54321);
    let test_message = b"RX TEST";

    let bytes = unsafe {
        sendto(fd, test_message.as_ptr(), test_message.len(), 0,
               &loopback_addr, std::mem::size_of::<SockAddrIn>() as u32)
    };
    if bytes < 0 {
        println!("UDP Socket Test: FAILED to send loopback packet, errno={}", -bytes);
        process::exit(6);
    }
    println!("UDP: Loopback packet sent, bytes={}", bytes);

    // Step 6: Try to receive the packet
    println!("UDP Socket Test: Attempting to receive packet...");
    let mut recv_buf = [0u8; 128];
    let mut src_addr = SockAddrIn::new([0, 0, 0, 0], 0);
    let mut addrlen: u32 = std::mem::size_of::<SockAddrIn>() as u32;

    let bytes = unsafe {
        recvfrom(rx_fd, recv_buf.as_mut_ptr(), recv_buf.len(), 0,
                 &mut src_addr, &mut addrlen)
    };
    if bytes < 0 {
        println!("UDP Socket Test: FAILED - recvfrom returned errno={}", -bytes);
        println!("UDP Socket Test: Loopback packet was not received.");
        println!("UDP Socket Test: This indicates a bug in the loopback delivery path.");
        process::exit(9);
    }
    println!("UDP: Received packet! bytes={}", bytes);

    // Verify the data matches what we sent
    let received_bytes = bytes as usize;
    if received_bytes == test_message.len() {
        let mut matches = true;
        for i in 0..received_bytes {
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

    // =========================================================================
    // Test 7: Ephemeral Port Test - bind(port=0) allocates a port
    // =========================================================================
    println!("UDP Ephemeral Port Test: Starting...");

    let ephemeral_fd = unsafe { socket(AF_INET, SOCK_DGRAM, 0) };
    if ephemeral_fd < 0 {
        println!("UDP Ephemeral Port Test: FAILED to create socket, errno={}", -ephemeral_fd);
        process::exit(10);
    }
    println!("UDP: Ephemeral socket created fd={}", ephemeral_fd);

    // Bind to port 0 - kernel should allocate an ephemeral port
    let ephemeral_addr = SockAddrIn::new([0, 0, 0, 0], 0);
    let ret = unsafe { bind(ephemeral_fd, &ephemeral_addr, std::mem::size_of::<SockAddrIn>() as u32) };
    if ret < 0 {
        println!("UDP Ephemeral Port Test: FAILED to bind to port 0, errno={}", -ret);
        process::exit(11);
    }
    println!("UDP_EPHEMERAL_TEST: port 0 bind OK");

    // Close ephemeral socket
    unsafe { close(ephemeral_fd); }

    // =========================================================================
    // Test 8: EADDRINUSE Test - binding to already-bound port fails
    // =========================================================================
    println!("UDP EADDRINUSE Test: Starting...");

    const EADDRINUSE: i32 = 98;

    let conflict_fd1 = unsafe { socket(AF_INET, SOCK_DGRAM, 0) };
    if conflict_fd1 < 0 {
        println!("UDP EADDRINUSE Test: FAILED to create socket1, errno={}", -conflict_fd1);
        process::exit(12);
    }

    // Bind first socket to port 54324
    let conflict_addr = SockAddrIn::new([0, 0, 0, 0], 54324);
    let ret = unsafe { bind(conflict_fd1, &conflict_addr, std::mem::size_of::<SockAddrIn>() as u32) };
    if ret < 0 {
        println!("UDP EADDRINUSE Test: FAILED to bind first socket, errno={}", -ret);
        process::exit(13);
    }
    println!("UDP: First socket bound to port 54324");

    // Create second socket and try to bind to same port
    let conflict_fd2 = unsafe { socket(AF_INET, SOCK_DGRAM, 0) };
    if conflict_fd2 < 0 {
        println!("UDP EADDRINUSE Test: FAILED to create socket2, errno={}", -conflict_fd2);
        process::exit(14);
    }

    let ret = unsafe { bind(conflict_fd2, &conflict_addr, std::mem::size_of::<SockAddrIn>() as u32) };
    if ret == 0 {
        println!("UDP EADDRINUSE Test: FAILED - second bind should have failed!");
        process::exit(15);
    } else if -ret == EADDRINUSE {
        println!("UDP_EADDRINUSE_TEST: conflict detected OK");
    } else {
        println!("UDP EADDRINUSE Test: FAILED - expected EADDRINUSE(98), got errno={}", -ret);
        process::exit(16);
    }

    // Close conflict sockets
    unsafe {
        close(conflict_fd1);
        close(conflict_fd2);
    }

    // =========================================================================
    // Test 9: EAGAIN Test - recvfrom on empty queue returns EAGAIN
    // =========================================================================
    println!("UDP EAGAIN Test: Starting...");

    const EAGAIN: i32 = 11;

    let eagain_fd = unsafe { socket(AF_INET, SOCK_DGRAM | SOCK_NONBLOCK, 0) };
    if eagain_fd < 0 {
        println!("UDP EAGAIN Test: FAILED to create socket, errno={}", -eagain_fd);
        process::exit(17);
    }

    // Bind to a port
    let eagain_addr = SockAddrIn::new([0, 0, 0, 0], 54325);
    let ret = unsafe { bind(eagain_fd, &eagain_addr, std::mem::size_of::<SockAddrIn>() as u32) };
    if ret < 0 {
        println!("UDP EAGAIN Test: FAILED to bind, errno={}", -ret);
        process::exit(18);
    }
    println!("UDP: EAGAIN test socket bound to port 54325");

    // Try to receive without any data sent - should return EAGAIN
    let mut eagain_buf = [0u8; 64];
    let mut eagain_src = SockAddrIn::new([0, 0, 0, 0], 0);
    let mut eagain_addrlen: u32 = std::mem::size_of::<SockAddrIn>() as u32;
    let recv_ret = unsafe {
        recvfrom(eagain_fd, eagain_buf.as_mut_ptr(), eagain_buf.len(), 0,
                 &mut eagain_src, &mut eagain_addrlen)
    };
    if recv_ret >= 0 {
        println!("UDP EAGAIN Test: FAILED - recvfrom should have returned EAGAIN!");
        process::exit(19);
    } else if -(recv_ret as i32) == EAGAIN {
        println!("UDP_EAGAIN_TEST: empty queue OK");
    } else {
        println!("UDP EAGAIN Test: FAILED - expected EAGAIN(11), got errno={}", -(recv_ret as i32));
        process::exit(20);
    }

    // Close eagain socket
    unsafe { close(eagain_fd); }

    // =========================================================================
    // Test 10: Multiple Packets Test - multiple packets queued and received
    // =========================================================================
    println!("UDP Multiple Packets Test: Starting...");

    // Create receiver socket
    let multi_rx_fd = unsafe { socket(AF_INET, SOCK_DGRAM, 0) };
    if multi_rx_fd < 0 {
        println!("UDP Multiple Packets Test: FAILED to create RX socket, errno={}", -multi_rx_fd);
        process::exit(21);
    }

    // Bind receiver to port 54326
    let multi_rx_addr = SockAddrIn::new([0, 0, 0, 0], 54326);
    let ret = unsafe { bind(multi_rx_fd, &multi_rx_addr, std::mem::size_of::<SockAddrIn>() as u32) };
    if ret < 0 {
        println!("UDP Multiple Packets Test: FAILED to bind RX socket, errno={}", -ret);
        process::exit(22);
    }
    println!("UDP: Multi-packet RX socket bound to port 54326");

    // Create sender socket
    let multi_tx_fd = unsafe { socket(AF_INET, SOCK_DGRAM, 0) };
    if multi_tx_fd < 0 {
        println!("UDP Multiple Packets Test: FAILED to create TX socket, errno={}", -multi_tx_fd);
        process::exit(23);
    }

    // Bind sender to a different port
    let multi_tx_addr = SockAddrIn::new([0, 0, 0, 0], 54327);
    let ret = unsafe { bind(multi_tx_fd, &multi_tx_addr, std::mem::size_of::<SockAddrIn>() as u32) };
    if ret < 0 {
        println!("UDP Multiple Packets Test: FAILED to bind TX socket, errno={}", -ret);
        process::exit(24);
    }
    println!("UDP: Multi-packet TX socket bound to port 54327");

    // Destination address (loopback to receiver)
    let multi_dest = SockAddrIn::new([10, 0, 2, 15], 54326);

    // Send 3 different packets
    let packets: [&[u8]; 3] = [b"PKT1", b"PKT2", b"PKT3"];
    for (i, pkt) in packets.iter().enumerate() {
        let bytes = unsafe {
            sendto(multi_tx_fd, pkt.as_ptr(), pkt.len(), 0,
                   &multi_dest, std::mem::size_of::<SockAddrIn>() as u32)
        };
        if bytes < 0 {
            println!("UDP Multiple Packets Test: FAILED to send packet {}, errno={}", i + 1, -bytes);
            process::exit(25);
        }
        println!("UDP: Sent packet {}, bytes={}", i + 1, bytes);
    }

    // Receive all 3 packets and verify data
    let mut multi_recv_buf = [0u8; 64];
    let mut multi_recv_src = SockAddrIn::new([0, 0, 0, 0], 0);
    let mut received_count = 0;

    for i in 0..3 {
        let mut multi_addrlen: u32 = std::mem::size_of::<SockAddrIn>() as u32;
        let bytes = unsafe {
            recvfrom(multi_rx_fd, multi_recv_buf.as_mut_ptr(), multi_recv_buf.len(), 0,
                     &mut multi_recv_src, &mut multi_addrlen)
        };
        if bytes < 0 {
            println!("UDP Multiple Packets Test: FAILED to receive packet {}, errno={}", i + 1, -bytes);
            continue;
        }
        println!("UDP: Received packet {}, bytes={}", i + 1, bytes);

        // Verify data matches expected packet
        let expected = packets[i];
        let received_bytes = bytes as usize;
        if received_bytes == expected.len() {
            let mut matches = true;
            for j in 0..received_bytes {
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

    if received_count == 3 {
        println!("UDP_MULTIPACKET_TEST: 3 packets OK");
    } else {
        println!("UDP Multiple Packets Test: FAILED - only received {} of 3 packets", received_count);
        process::exit(26);
    }

    // Close multi-packet sockets
    unsafe {
        close(multi_rx_fd);
        close(multi_tx_fd);
    }

    // Close the original test sockets
    unsafe {
        close(fd);
        close(rx_fd);
    }

    println!("UDP Socket Test: All tests passed!");
    process::exit(0);
}
