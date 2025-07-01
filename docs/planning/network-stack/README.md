# Network Stack Planning

## The Dream: Sending Our First Packet! 🎉

Imagine the day when Breenix can:
```bash
breenix> ping 8.8.8.8
PING 8.8.8.8: 64 bytes
Reply from 8.8.8.8: bytes=64 time=10ms
```

This will be a HUGE milestone!

## Implementation Roadmap

### Phase 1: Basic Ethernet Driver
**Goal**: Send and receive raw Ethernet frames

1. **Choose Initial NIC**
   - e1000 (Intel) - QEMU default, well documented
   - RTL8139 - Simpler, good for learning
   - Start with e1000 since QEMU provides it by default

2. **Driver Basics**
   ```rust
   pub struct E1000 {
       mmio_base: VirtAddr,
       rx_ring: RxDescriptorRing,
       tx_ring: TxDescriptorRing,
   }
   
   impl E1000 {
       pub fn send_packet(&mut self, data: &[u8]) -> Result<(), Error> {
           // Place in TX ring
           // Kick hardware
       }
   }
   ```

3. **First Milestone**: See our MAC address!
   ```
   Network device initialized: 52:54:00:12:34:56
   ```

### Phase 2: ARP - Address Resolution Protocol
**Goal**: Resolve IP addresses to MAC addresses

```rust
pub async fn arp_resolve(ip: Ipv4Addr) -> Result<MacAddr, Error> {
    // Send ARP request
    // Wait for reply
    // Cache result
}
```

**Test**: Successfully resolve gateway MAC address

### Phase 3: IP Layer
**Goal**: Send and receive IP packets

- IPv4 header parsing/building
- Basic routing (just default gateway)
- Fragmentation (later)

### Phase 4: ICMP - Our First Ping!
**Goal**: This is THE moment! 🎉

```rust
pub async fn ping(dest: Ipv4Addr) -> Result<Duration, Error> {
    let packet = IcmpEchoRequest::new();
    let start = Instant::now();
    
    network.send(dest, packet).await?;
    let reply = network.receive_icmp_reply().await?;
    
    Ok(start.elapsed())
}
```

### Phase 5: UDP
**Goal**: Simple datagram communication

- Socket abstraction
- Port management
- Basic DNS queries

### Phase 6: TCP/IP
**Goal**: Real network communication

- Three-way handshake
- Sequence numbers
- Congestion control
- Socket API

## Hardware Setup in QEMU

```bash
# Enable e1000 network device
qemu-system-x86_64 \
    -netdev user,id=net0 \
    -device e1000,netdev=net0
```

## Exciting Milestones Along the Way

1. **"Link is up!"** - Detecting network cable
2. **First packet received** - Even if we can't parse it yet
3. **ARP reply** - Someone acknowledged we exist!
4. **First ICMP echo reply** - WE CAN PING! 🎉
5. **First TCP connection** - Maybe to a simple HTTP server?
6. **Download a file** - Using our own TCP/IP stack!

## Why This Is So Exciting

- **Visible Progress**: Each layer works independently
- **Real-World Interaction**: Talk to actual Internet hosts
- **Learning Experience**: Understand how Internet really works
- **Practical Use**: Could eventually self-update, send telemetry

## Development Tips

1. **Packet Capture**: Use Wireshark to see what we're sending
2. **Test Network**: QEMU user mode is perfect for development
3. **Start Simple**: Just getting link status is an achievement
4. **Celebrate Milestones**: Each working protocol is huge!

## Resources

- Intel e1000 manual (detailed but complete)
- OSDev Wiki networking section
- Linux driver source (for reference)
- RFC 791 (IP), RFC 792 (ICMP), RFC 826 (ARP)

## The First Ping

When we successfully ping 8.8.8.8, we should:
1. Take a screenshot
2. Frame the packet capture
3. Celebrate! 🍾

This will mean Breenix is truly talking to the world!