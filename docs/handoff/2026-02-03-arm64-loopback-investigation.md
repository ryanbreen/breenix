# ARM64 TCP/UDP Loopback Investigation

**Date**: 2026-02-03
**Session Focus**: Investigating why TCP/UDP loopback networking does not work on ARM64
**Result**: Root cause identified - TCP/UDP packet handling is disabled at compile-time for ARM64

---

## Executive Summary

The investigation revealed that TCP and UDP packet handling is intentionally disabled for ARM64 via `#[cfg(target_arch = "x86_64")]` conditional compilation attributes. This is not a bug in the loopback implementation but rather a deliberate (or incomplete) architectural decision that prevents any TCP/UDP traffic from being processed on ARM64.

**Key Finding**: Unix domain sockets work on ARM64 because they do not go through the network stack at all - they use a completely separate in-memory implementation.

---

## Problem Statement

### Symptoms

| Test | x86-64 | ARM64 | Notes |
|------|--------|-------|-------|
| `unix_socket_test` | PASS | PASS | 18 phases complete |
| `udp_socket_test` | PASS | HANG | Hangs when trying to use 127.0.0.1 |
| `tcp_socket_test` | PASS | HANG | Hangs when trying to use 127.0.0.1 |
| `tcp_blocking_test` | PASS | HANG | Hangs when trying to use 127.0.0.1 |

### User's Initial Hypotheses

1. Loopback interface might be missing
2. Packet routing might not deliver to loopback
3. Socket binding issue

---

## Root Cause Analysis

### The Actual Problem

**Location**: `kernel/src/net/ipv4.rs` (lines 171-189)

```rust
/// Handle an incoming IPv4 packet
pub fn handle_ipv4(eth_frame: &EthernetFrame, ip: &Ipv4Packet) {
    let config = super::config();

    // Check if this packet is for us (accept our IP or loopback addresses)
    if ip.dst_ip != config.ip_addr && ip.dst_ip[0] != 127 {
        // Not for us, ignore (we don't do routing)
        return;
    }

    match ip.protocol {
        PROTOCOL_ICMP => {
            if let Some(icmp_packet) = icmp::IcmpPacket::parse(ip.payload) {
                icmp::handle_icmp(eth_frame, ip, &icmp_packet);
            }
        }
        #[cfg(target_arch = "x86_64")]  // <-- TCP DISABLED ON ARM64
        PROTOCOL_TCP => {
            super::tcp::handle_tcp(ip, ip.payload);
        }
        #[cfg(target_arch = "x86_64")]  // <-- UDP DISABLED ON ARM64
        PROTOCOL_UDP => {
            super::udp::handle_udp(ip, ip.payload);
        }
        _ => {
            #[cfg(target_arch = "x86_64")]
            log::debug!("IPv4: Unknown protocol {}", ip.protocol);
        }
    }
}
```

**What Happens on ARM64**:

1. The loopback detection in `send_ipv4()` correctly identifies loopback packets (lines 353-378 in `kernel/src/net/mod.rs`)
2. Loopback packets are correctly queued to `LOOPBACK_QUEUE`
3. `drain_loopback_queue()` is called and correctly parses packets
4. `handle_ipv4()` is called to process the packet
5. **TCP/UDP protocol matching falls through to `_ => {}` because the match arms are conditionally compiled out**
6. Packet is silently dropped - no handler is called

### Why This Causes Hangs (Not Errors)

The socket syscalls (`connect`, `sendto`, `recvfrom`) on ARM64 work correctly:

- `socket()` creates a socket successfully
- `bind()` binds the socket successfully
- `sendto()` queues the packet to the loopback queue successfully
- `recvfrom()` blocks waiting for a packet that will never arrive

The blocking behavior is correct - the syscall is waiting for a response. But since the packet is never processed, no response ever comes, and the thread blocks forever.

### Loopback Mechanism is Correctly Implemented

The loopback mechanism in `kernel/src/net/mod.rs` is architecture-independent and works correctly:

```rust
/// Send an IPv4 packet
pub fn send_ipv4(dst_ip: [u8; 4], protocol: u8, payload: &[u8]) -> Result<(), &'static str> {
    let config = config();

    // Check for loopback - sending to ourselves or to 127.x.x.x network
    if dst_ip == config.ip_addr || dst_ip[0] == 127 {
        net_debug!("NET: Loopback detected, queueing packet for deferred delivery");

        // Build IP packet
        let ip_packet = ipv4::Ipv4Packet::build(
            config.ip_addr,
            dst_ip,
            protocol,
            payload,
        );

        // Queue for deferred delivery
        let mut queue = LOOPBACK_QUEUE.lock();
        if queue.len() >= MAX_LOOPBACK_QUEUE_SIZE {
            queue.remove(0);
            net_warn!("NET: Loopback queue full, dropped oldest packet");
        }
        queue.push(LoopbackPacket { data: ip_packet });

        return Ok(());
    }
    // ... rest for real network ...
}
```

And `drain_loopback_queue()` correctly calls `handle_ipv4()`:

```rust
pub fn drain_loopback_queue() {
    let packets: Vec<LoopbackPacket> = {
        let mut queue = LOOPBACK_QUEUE.lock();
        core::mem::take(&mut *queue)
    };

    for packet in packets {
        if let Some(parsed_ip) = ipv4::Ipv4Packet::parse(&packet.data) {
            let src_mac = get_mac_address().unwrap_or([0; 6]);
            let dummy_frame = ethernet::EthernetFrame {
                src_mac,
                dst_mac: src_mac,
                ethertype: ethernet::ETHERTYPE_IPV4,
                payload: &packet.data,
            };
            ipv4::handle_ipv4(&dummy_frame, &parsed_ip);  // This works!
        }
    }
}
```

---

## Why Unix Sockets Work on ARM64

Unix domain sockets bypass the entire network stack:

1. **No IP/TCP/UDP**: Unix sockets use `FdKind::UnixSocket`, `FdKind::UnixStream`, `FdKind::UnixListener`
2. **In-memory buffers**: Data passes through `VecDeque<u8>` buffers, not network packets
3. **Registry-based**: `UNIX_SOCKET_REGISTRY` maps paths to listeners - no routing needed
4. **No conditional compilation**: The entire Unix socket implementation in `kernel/src/socket/unix.rs` is architecture-independent

This is why `unix_socket_test` passes all 18 phases on ARM64.

---

## Comparison: x86-64 vs ARM64 Network Stack

| Component | x86-64 | ARM64 |
|-----------|--------|-------|
| Network driver | E1000 (PCI) | VirtIO-net (MMIO) |
| ARP handling | Yes | Yes |
| ICMP (ping) | Yes | Yes |
| TCP handling | **Yes** | **No** |
| UDP handling | **Yes** | **No** |
| Loopback queue | Yes | Yes |
| Unix domain sockets | Yes | Yes |

---

## Recommended Fix

### Minimal Fix (Enable TCP/UDP on ARM64)

**File**: `kernel/src/net/ipv4.rs`

Remove the `#[cfg(target_arch = "x86_64")]` attributes:

```rust
match ip.protocol {
    PROTOCOL_ICMP => {
        if let Some(icmp_packet) = icmp::IcmpPacket::parse(ip.payload) {
            icmp::handle_icmp(eth_frame, ip, &icmp_packet);
        }
    }
    // Remove cfg attribute:
    PROTOCOL_TCP => {
        super::tcp::handle_tcp(ip, ip.payload);
    }
    // Remove cfg attribute:
    PROTOCOL_UDP => {
        super::udp::handle_udp(ip, ip.payload);
    }
    _ => {
        log::debug!("IPv4: Unknown protocol {}", ip.protocol);
    }
}
```

### Additional Considerations

1. **Logging**: The `log::debug!` calls in `tcp.rs` and `udp.rs` use `log::*` macros which may need ARM64 equivalents or the existing `net_log!` macro wrapper

2. **Testing**: The `tcp.rs` and `udp.rs` implementations themselves are architecture-independent. They should work on ARM64 once the `handle_ipv4()` dispatch is enabled.

3. **Network device**: ARM64 uses VirtIO-net which is already initialized and working (ARP and ICMP work). No driver changes needed.

4. **Real network testing**: This fix enables loopback. For real network connectivity to external hosts, QEMU's SLIRP networking also needs to work with VirtIO-net, which requires separate verification.

---

## Files Relevant to This Issue

| File | Purpose |
|------|---------|
| `kernel/src/net/ipv4.rs` | **Root cause** - conditional TCP/UDP dispatch |
| `kernel/src/net/mod.rs` | Loopback queue implementation (works correctly) |
| `kernel/src/net/tcp.rs` | TCP state machine (architecture-independent) |
| `kernel/src/net/udp.rs` | UDP packet handling (architecture-independent) |
| `kernel/src/syscall/socket.rs` | Socket syscalls (architecture-independent) |
| `kernel/src/socket/unix.rs` | Unix sockets (works on ARM64) |

---

## Verification Plan

After implementing the fix:

1. **Build ARM64 kernel**:
   ```bash
   cargo build --release --features testing --target aarch64-breenix.json \
       -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
       -p kernel --bin kernel-aarch64
   ```

2. **Run socket tests**:
   ```bash
   ./docker/qemu/run-aarch64-test-suite.sh udp_socket_test
   ./docker/qemu/run-aarch64-test-suite.sh tcp_socket_test
   ./docker/qemu/run-aarch64-test-suite.sh tcp_blocking_test
   ```

3. **Expected results**: All three tests should pass (or at least progress past the hang point)

---

## Risk Assessment

| Risk | Likelihood | Mitigation |
|------|------------|------------|
| TCP/UDP code has x86-specific bugs | Low | Code appears architecture-independent |
| Log macro differences cause issues | Medium | Replace with `net_log!` or serial_println! |
| Lock contention differences | Low | Same spinlock primitives on both archs |
| Timing differences expose races | Medium | Run stress tests after enabling |

---

## Conclusion

The TCP/UDP loopback issue on ARM64 is not a fundamental architectural problem but simply a feature that was never enabled. The network stack, loopback queue, socket syscalls, and protocol implementations are all architecture-independent. The fix is straightforward: remove two `#[cfg]` attributes.

The fact that Unix domain sockets work perfectly confirms that the socket syscall infrastructure, blocking I/O, and process/thread management all function correctly on ARM64.

---

*Document generated: 2026-02-03*
*Author: Claude Code (Opus 4.5)*
