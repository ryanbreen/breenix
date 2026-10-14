//! Scheduler and shared-object boundaries for the production Unix protocol.
pub mod object {
    use crate::{addr::SockAddr, ipc::fd::FileDescriptor, proto::UnixSock};
    use std::sync::{
        atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering},
        Arc, Condvar, Mutex,
    };
    pub struct UCred {
        pub pid: u32,
        pub uid: u32,
        pub gid: u32,
    }
    pub fn caller_ucred() -> UCred {
        UCred {
            pid: 1,
            uid: 2,
            gid: 3,
        }
    }
    pub struct Socket {
        pub family: u16,
        pub kind: u16,
        pub protocol: u16,
        pub common: SocketCommon,
        pub proto: SocketProto,
    }
    pub enum SocketProto {
        Unix(UnixSock),
        Other,
    }
    impl SocketProto {
        pub fn detach(&mut self) -> Self {
            match self {
                Self::Unix(unix) => Self::Unix(unix.detach()),
                Self::Other => Self::Other,
            }
        }
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
                proto,
                common: SocketCommon::new(),
            }))
        }
        pub fn ops(&self) -> &dyn ProtoOps {
            match &self.proto {
                SocketProto::Unix(s) => s,
                _ => unreachable!(),
            }
        }
    }
    impl Drop for Socket {
        fn drop(&mut self) {
            self.ops().release(self)
        }
    }
    pub struct SocketCommon {
        shut: AtomicU8,
        pub seq: AtomicU64,
        pub waiting: AtomicBool,
        pub gate: Mutex<()>,
        pub changed: Condvar,
    }
    impl SocketCommon {
        pub fn new() -> Self {
            Self {
                shut: AtomicU8::new(0),
                seq: AtomicU64::new(0),
                waiting: AtomicBool::new(false),
                gate: Mutex::new(()),
                changed: Condvar::new(),
            }
        }
        pub fn shut_rd(&self) -> bool {
            self.shut.load(Ordering::Acquire) & 1 != 0
        }
        pub fn shut_wr(&self) -> bool {
            self.shut.load(Ordering::Acquire) & 2 != 0
        }
        pub fn set_shut(&self, how: u8) {
            self.shut.fetch_or(
                match how {
                    0 => 1,
                    1 => 2,
                    _ => 3,
                },
                Ordering::Release,
            );
        }
        pub fn notify(&self) {
            let _guard = self.gate.lock().unwrap();
            self.seq.fetch_add(1, Ordering::Release);
            self.changed.notify_all();
        }
        pub fn snd_deadline(&self) -> Option<u64> {
            None
        }
        pub fn rcv_deadline(&self) -> Option<u64> {
            None
        }
        pub fn sndbuf(&self) -> usize {
            65536
        }
    }
    pub struct SendMsg<'a> {
        pub data: &'a [u8],
        pub dest: Option<SockAddr>,
        pub flags: u32,
        pub nonblock: bool,
        pub rights: Vec<FileDescriptor>,
        pub cred: UCred,
    }
    pub struct RecvMsg {
        pub cap: usize,
        pub data: Vec<u8>,
        pub flags: u32,
        pub nonblock: bool,
        pub out_flags: u32,
        pub from: Option<SockAddr>,
        pub rights: Vec<FileDescriptor>,
    }
    pub struct OptBuf {
        pub bytes: [u8; 16],
        pub len: usize,
    }
    pub trait ProtoOps: Send + Sync {
        fn bind(&self, sk: &Arc<Socket>, addr: &SockAddr) -> Result<(), i32>;
        fn listen(&self, sk: &Arc<Socket>, backlog: i32) -> Result<(), i32>;
        fn connect(&self, sk: &Arc<Socket>, addr: &SockAddr, nonblock: bool) -> Result<(), i32>;
        fn accept(&self, sk: &Arc<Socket>, nonblock: bool) -> Result<(Arc<Socket>, SockAddr), i32>;
        fn sendmsg(&self, sk: &Arc<Socket>, msg: &mut SendMsg) -> Result<usize, i32>;
        fn recvmsg(&self, sk: &Arc<Socket>, msg: &mut RecvMsg) -> Result<usize, i32>;
        fn recvmsg_with(
            &self,
            sk: &Arc<Socket>,
            msg: &mut RecvMsg,
            deliver: &mut dyn FnMut(&mut RecvMsg) -> Result<(), i32>,
        ) -> Result<usize, i32>;
        fn shutdown(&self, sk: &Arc<Socket>, how: u8) -> Result<(), i32>;
        fn local_name(&self, sk: &Socket) -> SockAddr;
        fn try_local_name(&self, sk: &Socket) -> Result<SockAddr, i32>;
        fn peer_name(&self, sk: &Socket) -> Result<SockAddr, i32>;
        fn poll(&self, sk: &Socket) -> u16;
        fn notifies(&self) -> bool {
            false
        }
        fn is_listening(&self, sk: &Socket) -> bool;
        fn getsockopt(
            &self,
            sk: &Socket,
            level: i32,
            name: i32,
            out: &mut OptBuf,
        ) -> Result<(), i32>;
        fn release(&self, sk: &Socket);
    }
}
pub mod wait {
    use std::sync::atomic::Ordering;
    pub fn wait_until<R>(
        sk: &crate::object::Socket,
        nonblock: bool,
        _deadline: Option<u64>,
        mut check: impl FnMut() -> Option<Result<R, i32>>,
    ) -> Result<R, i32> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let seq = sk.common.seq.load(Ordering::Acquire);
            if let Some(result) = check() {
                sk.common.waiting.store(false, Ordering::Release);
                return result;
            }
            if nonblock {
                return Err(11);
            }
            sk.common.waiting.store(true, Ordering::Release);
            let mut guard = sk.common.gate.lock().unwrap();
            while sk.common.seq.load(Ordering::Acquire) == seq {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                let (next, timed) = sk.common.changed.wait_timeout(guard, remaining).unwrap();
                guard = next;
                if timed.timed_out() {
                    sk.common.waiting.store(false, Ordering::Release);
                    return Err(11);
                }
            }
        }
    }
}

pub fn make_socket(kind: u16) -> std::sync::Arc<object::Socket> {
    object::Socket::new(
        1,
        kind,
        0,
        object::SocketProto::Unix(crate::proto::UnixSock::new(kind)),
    )
    .unwrap()
}
