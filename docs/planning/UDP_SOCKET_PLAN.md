# UDP Socket Implementation Plan

## Overview

Implement UDP sockets for Breenix, enabling userspace programs to send and receive UDP datagrams. This is the foundation for DNS clients, NTP sync, DHCP, and other UDP-based protocols.

## Architecture

### Components to Build

```
kernel/src/socket/
├── mod.rs          # Socket module, FD table, syscall routing
├── udp.rs          # UDP socket implementation
└── types.rs        # SocketAddr, Protocol enums

kernel/src/net/
├── udp.rs          # UDP packet parsing/construction (NEW)
└── mod.rs          # Integration with UDP handler (MODIFY)

kernel/src/syscall/
├── mod.rs          # Add socket syscall numbers (MODIFY)
├── dispatcher.rs   # Route socket syscalls (MODIFY)
└── socket.rs       # Socket syscall handlers (NEW)

kernel/src/process/
└── process.rs      # Add FD table to Process (MODIFY)
```

### Data Flow

**TX Path (sendto):**
```
userspace sendto(fd, buf, addr, port)
    → sys_sendto validates args
    → socket lookup by fd
    → build UDP header
    → ipv4::build_packet(protocol=17)
    → net::send_ipv4()
    → e1000::transmit()
```

**RX Path (recvfrom):**
```
e1000 interrupt → RX descriptor ready
    → net::process_rx()
    → ipv4::handle_ipv4() sees protocol=17
    → udp::handle_udp()
    → match (dst_port) to socket
    → enqueue to socket rx_queue
    → wake blocked recvfrom
```

## Implementation Phases

### Phase 1: File Descriptor Infrastructure

**Goal:** Add FD table to Process struct, implement basic FD allocation.

**Files:**
- `kernel/src/socket/mod.rs` - Create module with FdTable
- `kernel/src/process/process.rs` - Add fd_table field

**Structures:**
```rust
pub enum FdKind {
    Stdin,
    Stdout,
    Stderr,
    Socket(SocketHandle),
}

pub struct FileDescriptor {
    pub kind: FdKind,
    pub flags: u32,  // O_NONBLOCK, etc.
}

pub struct FdTable {
    table: [Option<FileDescriptor>; 64],  // Fixed size for simplicity
}

impl FdTable {
    pub fn new() -> Self;
    pub fn alloc(&mut self, fd: FileDescriptor) -> Option<u32>;
    pub fn get(&self, fd: u32) -> Option<&FileDescriptor>;
    pub fn close(&mut self, fd: u32) -> Result<(), i32>;
}
```

**Process changes:**
```rust
pub struct Process {
    // ... existing fields ...
    pub fd_table: FdTable,  // NEW
}
```

**Deliverable:** FD table infrastructure, no syscalls yet.

### Phase 2: Socket Structures

**Goal:** Define socket types and UDP socket state.

**Files:**
- `kernel/src/socket/types.rs` - Socket address types
- `kernel/src/socket/udp.rs` - UDP socket implementation

**Structures:**
```rust
// types.rs
#[repr(C)]
pub struct SockAddrIn {
    pub family: u16,      // AF_INET = 2
    pub port: u16,        // Network byte order
    pub addr: [u8; 4],    // IPv4 address
    pub zero: [u8; 8],    // Padding
}

// udp.rs
pub struct UdpSocket {
    pub local_addr: Option<[u8; 4]>,
    pub local_port: Option<u16>,
    pub bound: bool,
    pub rx_queue: VecDeque<UdpPacket>,  // Received packets
}

pub struct UdpPacket {
    pub src_addr: [u8; 4],
    pub src_port: u16,
    pub data: Vec<u8>,
}

impl UdpSocket {
    pub fn new() -> Self;
    pub fn bind(&mut self, addr: [u8; 4], port: u16) -> Result<(), i32>;
    pub fn send_to(&self, data: &[u8], addr: [u8; 4], port: u16) -> Result<usize, i32>;
    pub fn recv_from(&mut self) -> Option<UdpPacket>;
}
```

**Deliverable:** Socket structures, no syscalls yet.

### Phase 3: UDP Packet Layer

**Goal:** Parse and construct UDP packets.

**Files:**
- `kernel/src/net/udp.rs` - UDP packet handling (NEW)
- `kernel/src/net/mod.rs` - Integrate UDP handler

**UDP Header (8 bytes):**
```
0       2       4       6       8
+-------+-------+-------+-------+
|src_port|dst_port| length|checksum|
+-------+-------+-------+-------+
```

**Implementation:**
```rust
// net/udp.rs
pub const UDP_HEADER_SIZE: usize = 8;

pub struct UdpHeader {
    pub src_port: u16,
    pub dst_port: u16,
    pub length: u16,
    pub checksum: u16,
}

impl UdpHeader {
    pub fn parse(data: &[u8]) -> Option<(Self, &[u8])>;
}

pub fn build_udp_packet(
    src_port: u16,
    dst_port: u16,
    payload: &[u8],
) -> Vec<u8>;

pub fn handle_udp(src_ip: [u8; 4], data: &[u8]);
```

**Integration in net/mod.rs:**
```rust
// In handle_ipv4():
match ip.protocol {
    PROTOCOL_ICMP => icmp::handle_icmp(...),
    PROTOCOL_UDP => udp::handle_udp(ip.src_ip, ip.payload),  // NEW
    _ => {}
}
```

**Deliverable:** UDP packets can be parsed and constructed.

### Phase 4: Socket Syscalls

**Goal:** Implement socket, bind, sendto, recvfrom, close.

**Syscall Numbers (Linux x86_64):**
```rust
pub const SYS_SOCKET: u64 = 41;
pub const SYS_BIND: u64 = 49;
pub const SYS_SENDTO: u64 = 44;
pub const SYS_RECVFROM: u64 = 45;
// SYS_CLOSE already exists as 3
```

**Files:**
- `kernel/src/syscall/mod.rs` - Add syscall numbers
- `kernel/src/syscall/dispatcher.rs` - Route to handlers
- `kernel/src/syscall/socket.rs` - Implement handlers (NEW)

**Syscall Signatures:**
```rust
// socket(domain, type, protocol) -> fd
// domain: AF_INET=2, type: SOCK_DGRAM=2, protocol: 0
pub fn sys_socket(domain: u64, sock_type: u64, protocol: u64) -> SyscallResult;

// bind(fd, addr, addrlen) -> 0 or -errno
pub fn sys_bind(fd: u64, addr: u64, addrlen: u64) -> SyscallResult;

// sendto(fd, buf, len, flags, dest_addr, addrlen) -> bytes_sent
pub fn sys_sendto(
    fd: u64, buf: u64, len: u64,
    flags: u64, dest_addr: u64, addrlen: u64
) -> SyscallResult;

// recvfrom(fd, buf, len, flags, src_addr, addrlen) -> bytes_recv
pub fn sys_recvfrom(
    fd: u64, buf: u64, len: u64,
    flags: u64, src_addr: u64, addrlen: u64
) -> SyscallResult;
```

**Error Codes:**
```rust
pub const EBADF: i32 = 9;      // Bad file descriptor
pub const ENOTSOCK: i32 = 88;  // Not a socket
pub const EAFNOSUPPORT: i32 = 97;  // Address family not supported
pub const EADDRINUSE: i32 = 98;    // Address already in use
pub const ENOTCONN: i32 = 107;     // Not connected
```

**Deliverable:** Socket syscalls work from kernel side.

### Phase 5: Socket Registry & RX Dispatch

**Goal:** Global socket registry for incoming packet dispatch.

**Problem:** When UDP packet arrives, need to find which socket (process) it belongs to based on destination port.

**Solution:** Global socket registry indexed by port.

```rust
// socket/mod.rs
static SOCKET_REGISTRY: Mutex<SocketRegistry> = Mutex::new(SocketRegistry::new());

pub struct SocketRegistry {
    // port -> (pid, socket_handle)
    udp_ports: BTreeMap<u16, (u64, SocketHandle)>,
}

impl SocketRegistry {
    pub fn bind_udp(&mut self, port: u16, pid: u64, handle: SocketHandle) -> Result<(), i32>;
    pub fn unbind_udp(&mut self, port: u16);
    pub fn lookup_udp(&self, port: u16) -> Option<(u64, SocketHandle)>;
}
```

**RX Flow:**
```rust
// net/udp.rs
pub fn handle_udp(src_ip: [u8; 4], data: &[u8]) {
    let header = UdpHeader::parse(data)?;

    // Look up socket by destination port
    if let Some((pid, handle)) = SOCKET_REGISTRY.lock().lookup_udp(header.dst_port) {
        // Enqueue packet to socket's rx_queue
        // Wake any blocked recvfrom
    }
}
```

**Deliverable:** Incoming UDP packets routed to correct socket.

### Phase 6: libbreenix Wrappers

**Goal:** Userspace API for sockets.

**Files:**
- `libs/libbreenix/src/socket.rs` - Socket syscall wrappers (NEW)
- `libs/libbreenix/src/lib.rs` - Export socket module

**API:**
```rust
// libbreenix/src/socket.rs
pub const AF_INET: u16 = 2;
pub const SOCK_DGRAM: u16 = 2;

#[repr(C)]
pub struct SockAddrIn {
    pub family: u16,
    pub port: u16,       // Network byte order!
    pub addr: [u8; 4],
    pub zero: [u8; 8],
}

pub fn socket(domain: i32, sock_type: i32, protocol: i32) -> i32;
pub fn bind(fd: i32, addr: &SockAddrIn) -> i32;
pub fn sendto(fd: i32, buf: &[u8], addr: &SockAddrIn) -> isize;
pub fn recvfrom(fd: i32, buf: &mut [u8], addr: &mut SockAddrIn) -> isize;

// Helper
pub fn htons(x: u16) -> u16 { x.to_be() }
pub fn ntohs(x: u16) -> u16 { u16::from_be(x) }
```

**Deliverable:** Clean userspace socket API.

### Phase 7: Test Program & Boot Stages

**Goal:** E2E test proving UDP works.

**Test Program:** `userspace/programs/udp_echo_test.rs`
```rust
// 1. Create UDP socket
// 2. Bind to port 12345
// 3. Send packet to gateway (QEMU host)
// 4. Receive echo response (if QEMU configured)
// OR: Just verify send succeeds and no crash
```

**Boot Stages to Add:**
```rust
BootStage {
    name: "UDP socket created",
    marker: "UDP: Socket created",
    ...
},
BootStage {
    name: "UDP socket bound",
    marker: "UDP: Socket bound to port",
    ...
},
BootStage {
    name: "UDP packet sent",
    marker: "UDP: Packet sent successfully",
    ...
},
```

**E2E Test Strategy:**
Since QEMU SLIRP doesn't echo UDP, test by:
1. Send UDP packet to known port
2. Verify transmit succeeds (packet left kernel)
3. For RX testing, could set up external UDP echo server or use QEMU's built-in TFTP/DNS

**Deliverable:** Proof that UDP sockets work end-to-end.

## Implementation Order

1. **Phase 1: FD Infrastructure** - Foundation for all socket work
2. **Phase 2: Socket Structures** - Define the data types
3. **Phase 3: UDP Packet Layer** - Parse/build UDP packets
4. **Phase 4: Socket Syscalls** - Kernel-side syscall handlers
5. **Phase 5: Socket Registry** - RX packet dispatch
6. **Phase 6: libbreenix** - Userspace wrappers
7. **Phase 7: Testing** - E2E verification

## Files Summary

**New Files:**
- `kernel/src/socket/mod.rs`
- `kernel/src/socket/types.rs`
- `kernel/src/socket/udp.rs`
- `kernel/src/net/udp.rs`
- `kernel/src/syscall/socket.rs`
- `libs/libbreenix/src/socket.rs`
- `userspace/programs/udp_echo_test.rs`

**Modified Files:**
- `kernel/src/lib.rs` - Add socket module
- `kernel/src/process/process.rs` - Add fd_table
- `kernel/src/net/mod.rs` - Integrate UDP
- `kernel/src/net/ipv4.rs` - Route protocol 17 to UDP
- `kernel/src/syscall/mod.rs` - Add syscall numbers
- `kernel/src/syscall/dispatcher.rs` - Route syscalls
- `libs/libbreenix/src/lib.rs` - Export socket

## Success Criteria

1. Build clean (0 warnings)
2. All existing boot stages pass (60/60)
3. New UDP boot stages pass
4. Userspace can: create socket, bind, sendto
5. UDP packet visible on QEMU network (tcpdump/wireshark)

## Risks & Mitigations

| Risk | Mitigation |
|------|------------|
| RX packet loss | Use VecDeque with bounded size, drop oldest |
| Port conflicts | Registry enforces unique bindings |
| Memory leaks | Socket cleanup on process exit |
| Blocking recvfrom | Start non-blocking, add blocking later |

## Open Questions

1. **Blocking vs non-blocking:** Start with non-blocking only (return EAGAIN if no data)?
2. **Socket cleanup:** On process exit, auto-close all sockets?
3. **Max sockets per process:** Fixed limit (e.g., 64) or dynamic?

**Recommendation:** Start simple:
- Non-blocking only (return EAGAIN)
- Auto-cleanup on exit
- Fixed 64 FDs per process
