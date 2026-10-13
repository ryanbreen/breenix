pub mod addr {
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum SockAddr {
        Unspec,
        Inet { addr: [u8; 4], port: u16 },
        Unix,
    }
}
pub mod object {
    use super::addr::SockAddr;
    use super::udp::UdpSock;
    use alloc::sync::Arc;
    use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicU8, AtomicUsize, Ordering};
    pub struct Socket {
        pub family: u16,
        pub kind: u16,
        pub protocol: u16,
        pub common: SocketCommon,
        pub proto: SocketProto,
    }
    pub enum SocketProto {
        Udp(UdpSock),
        Tcp,
        Unix,
    }
    impl Socket {
        pub fn new(
            family: u16,
            kind: u16,
            protocol: u16,
            proto: SocketProto,
        ) -> Result<Arc<Self>, i32> {
            Ok(Arc::new(Self {
                family,
                kind,
                protocol,
                common: SocketCommon::new(),
                proto,
            }))
        }
        pub fn ops(&self) -> &dyn ProtoOps {
            match &self.proto {
                SocketProto::Udp(udp) => udp,
                _ => unreachable!(),
            }
        }
    }
    impl Drop for Socket {
        fn drop(&mut self) {
            if let SocketProto::Udp(udp) = &mut self.proto {
                let detached = udp.detach();
                detached.release(self);
            }
        }
    }
    pub struct SocketCommon {
        error: AtomicI32,
        shut: AtomicU8,
        seq: AtomicU64,
        pub reuse: AtomicBool,
        pub broadcast: AtomicBool,
        pub rcvbuf: AtomicUsize,
    }
    impl SocketCommon {
        pub fn new() -> Self {
            Self {
                error: AtomicI32::new(0),
                shut: AtomicU8::new(0),
                seq: AtomicU64::new(0),
                reuse: AtomicBool::new(false),
                broadcast: AtomicBool::new(false),
                rcvbuf: AtomicUsize::new(212992),
            }
        }
        pub fn set_error(&self, e: i32) {
            self.error.store(e, Ordering::Release)
        }
        pub fn take_error(&self) -> i32 {
            self.error.swap(0, Ordering::AcqRel)
        }
        pub fn peek_error(&self) -> i32 {
            self.error.load(Ordering::Acquire)
        }
        pub fn set_shut(&self, how: u8) {
            self.shut.fetch_or(
                match how {
                    0 => 1,
                    1 => 2,
                    _ => 3,
                },
                Ordering::AcqRel,
            );
        }
        pub fn shut_rd(&self) -> bool {
            self.shut.load(Ordering::Acquire) & 1 != 0
        }
        pub fn shut_wr(&self) -> bool {
            self.shut.load(Ordering::Acquire) & 2 != 0
        }
        pub fn notify(&self) {
            self.seq.fetch_add(1, Ordering::AcqRel);
        }
        pub fn reuseaddr(&self) -> bool {
            self.reuse.load(Ordering::Acquire)
        }
        pub fn broadcast(&self) -> bool {
            self.broadcast.load(Ordering::Acquire)
        }
        pub fn rcvbuf(&self) -> usize {
            self.rcvbuf.load(Ordering::Acquire)
        }
        pub fn rcv_deadline(&self) -> Option<u64> {
            None
        }
    }
    impl Default for SocketCommon {
        fn default() -> Self {
            Self::new()
        }
    }
    pub trait ProtoOps: Send + Sync {
        fn notifies(&self) -> bool {
            false
        }
        fn bind(&self, sk: &Arc<Socket>, addr: &SockAddr) -> Result<(), i32>;
        fn connect(&self, sk: &Arc<Socket>, addr: &SockAddr, nonblock: bool) -> Result<(), i32>;
        fn sendmsg(&self, sk: &Arc<Socket>, msg: &mut SendMsg) -> Result<usize, i32>;
        fn recvmsg(&self, sk: &Arc<Socket>, msg: &mut RecvMsg) -> Result<usize, i32>;
        fn shutdown(&self, sk: &Arc<Socket>, how: u8) -> Result<(), i32>;
        fn local_name(&self, sk: &Socket) -> SockAddr;
        fn peer_name(&self, sk: &Socket) -> Result<SockAddr, i32>;
        fn poll(&self, sk: &Socket) -> u16;
        fn release(&self, sk: &Socket);
    }
    pub struct SendMsg<'a> {
        pub data: &'a [u8],
        pub dest: Option<SockAddr>,
        pub flags: u32,
        pub nonblock: bool,
        pub rights: Vec<()>,
        pub cred: (),
    }
    pub struct RecvMsg {
        pub cap: usize,
        pub data: Vec<u8>,
        pub flags: u32,
        pub nonblock: bool,
        pub out_flags: u32,
        pub from: Option<SockAddr>,
        pub rights: Vec<()>,
    }
}
pub mod wait {
    pub fn wait_until<R>(
        _: &super::object::Socket,
        nonblock: bool,
        deadline: Option<u64>,
        mut check: impl FnMut() -> Option<Result<R, i32>>,
    ) -> Result<R, i32> {
        assert!(
            nonblock && deadline.is_none(),
            "host harness cannot certify blocking waits"
        );
        check().unwrap_or(Err(11))
    }
}
#[path = "../../../kernel/src/socket/udp.rs"]
pub mod udp;
