extern crate self as libbreenix;

use std::cell::RefCell;
pub use errno::Errno;
pub use error::Error;
use io::poll_events::*;
pub use ssh::{SshBuf, SshError, *};
use ssh::transport::ServerSession;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Fd(u64);
impl Fd { pub fn from_raw(n: u64) -> Self { Self(n) } }
const MASTER: Fd = Fd(3);
const SOCKET: Fd = Fd(4);
thread_local! {
    static STEP: RefCell<usize> = const { RefCell::new(0) };
    static NONBLOCK: RefCell<bool> = const { RefCell::new(false) };
    static WRITES: RefCell<Vec<Vec<u8>>> = const { RefCell::new(Vec::new()) };
    static SIZES: RefCell<Vec<(u16, u16)>> = const { RefCell::new(Vec::new()) };
}
pub mod errno { pub enum Errno { EAGAIN, EINTR, EIO, EBADF } }
pub mod error { pub enum Error { Os(super::errno::Errno) } }
pub mod io {
    use super::*;
    use errno::Errno;
    use error::Error;
    pub mod status_flags { pub const O_NONBLOCK: i32 = 4; }
    pub mod poll_events {
        pub const POLLIN: i16 = 1;
        pub const POLLOUT: i16 = 4;
        pub const POLLERR: i16 = 8;
        pub const POLLHUP: i16 = 16;
        pub const POLLNVAL: i16 = 32;
        pub const POLLRDHUP: i16 = 0x2000;
    }
    pub struct PollFd { fd: Fd, events: i16, pub revents: i16 }
    impl PollFd {
        pub fn new(fd: Fd, events: i16) -> Self { Self { fd, events, revents: 0 } }
    }
    pub fn poll(fds: &mut [PollFd], _: i32) -> Result<usize, Error> {
        let step = STEP.with_borrow_mut(|n| { *n += 1; *n });
        assert!(step <= 9, "relay did not exit on socket hangup");
        for f in fds {
            f.revents = match f.fd {
                SOCKET if step == 9 => POLLHUP,
                SOCKET => f.events & (POLLIN | POLLOUT),
                MASTER if step >= 6 => f.events & (POLLIN | POLLOUT),
                _ => 0,
            };
        }
        Ok(1)
    }
    pub fn fcntl_getfl(_: Fd) -> Result<u32, Error> { Ok(0) }
    pub fn fcntl_setfl(fd: Fd, flags: i32) -> Result<(), Error> {
        assert!(fd == MASTER);
        NONBLOCK.with_borrow_mut(|value| *value = flags & status_flags::O_NONBLOCK != 0);
        Ok(())
    }
    pub fn read(fd: Fd, out: &mut [u8]) -> Result<usize, Error> {
        assert!(fd == MASTER);
        if STEP.with_borrow(|n| *n == 6) {
            out[0] = b'x';
            Ok(1)
        } else {
            Err(Error::Os(Errno::EAGAIN))
        }
    }
    pub fn write(fd: Fd, data: &[u8]) -> Result<usize, Error> {
        assert!(fd == MASTER);
        assert!(NONBLOCK.with_borrow(|value| *value), "full PTY would block the SSH relay");
        let step = STEP.with_borrow(|n| *n);
        if step < 6 { return Err(Error::Os(Errno::EAGAIN)); }
        let n = data.len().min(2);
        WRITES.with_borrow_mut(|writes| writes.push(data[..n].to_vec()));
        Ok(n)
    }
    pub fn close(_: Fd) -> Result<(), Error> { Ok(()) }
}
pub mod termios {
    use super::*;
    #[derive(Default)]
    pub struct Winsize { pub ws_row: u16, pub ws_col: u16 }
    pub fn set_winsize(fd: Fd, size: &Winsize) -> Result<(), error::Error> {
        assert!(fd == MASTER);
        SIZES.with_borrow_mut(|sizes| sizes.push((size.ws_col, size.ws_row)));
        Ok(())
    }
}

pub mod ssh {
    use super::*;
    // @CONSTANTS@
    // @BUFFER@
    #[derive(Debug)]
    pub enum SshError { Disconnected, Io, Protocol(&'static str), ChannelNotFound }
    pub struct Channel {
        pub local_id: u32,
        pub remote_id: u32,
        pub recv_window: u32,
        pub send_window: u32,
        pub eof_received: bool,
        pub closed: bool,
        pub pty_term: Option<String>,
        pub pty_size: (u32, u32),
    }
    pub struct PacketIo {
        pub input: std::collections::VecDeque<Vec<u8>>,
        pub sent: Vec<Vec<u8>>,
    }
    impl PacketIo {
        pub fn fd(&self) -> Fd { SOCKET }
        pub fn send_packet(&mut self, packet: &[u8]) -> Result<(), ()> {
            self.sent.push(packet.to_vec());
            Ok(())
        }
        pub fn recv_packet(&mut self) -> Result<Vec<u8>, ()> {
            self.input.pop_front().ok_or(())
        }
        pub fn start_relay(&mut self) -> Result<(), ()> { Ok(()) }
        pub fn finish_relay(&mut self) -> Result<(), ()> { Ok(()) }
        pub fn flush_relay(&mut self) -> Result<(), ()> { Ok(()) }
        pub fn relay_output_pending(&self) -> bool { false }
        pub fn relay_can_queue(&self) -> bool { true }
        pub fn packet_ready(&mut self) -> bool { false }
    }
    pub mod channel {
        use super::*;
        // @CHANNEL_TYPES@
        // @GLOBAL_REQUEST@
        // @CHANNEL_REQUEST@
        pub fn server_handle_channel_open(_: &mut PacketIo, _: &[u8], _: Option<u32>) -> Result<Option<Channel>, SshError> {
            unreachable!("the relay fixture starts with an open channel")
        }
        pub fn send_window_adjust(io: &mut PacketIo, ch: &mut Channel, n: u32) -> Result<(), SshError> {
            let mut packet = vec![SSH_MSG_CHANNEL_WINDOW_ADJUST];
            SshBuf::put_u32(&mut packet, ch.remote_id);
            SshBuf::put_u32(&mut packet, n);
            io.send_packet(&packet).unwrap();
            ch.recv_window += n;
            Ok(())
        }
        pub fn send_channel_close(_: &mut PacketIo, _: &mut Channel) -> Result<(), SshError> { Ok(()) }
        pub fn send_channel_success(io: &mut PacketIo, ch: &Channel) -> Result<(), SshError> {
            channel_reply(io, ch, SSH_MSG_CHANNEL_SUCCESS)
        }
        pub fn send_channel_failure(io: &mut PacketIo, ch: &Channel) -> Result<(), SshError> {
            channel_reply(io, ch, SSH_MSG_CHANNEL_FAILURE)
        }
        fn channel_reply(io: &mut PacketIo, ch: &Channel, kind: u8) -> Result<(), SshError> {
            let mut packet = vec![kind];
            SshBuf::put_u32(&mut packet, ch.remote_id);
            io.send_packet(&packet).map_err(|_| SshError::Io)
        }
    }

    pub mod transport {
        use super::*;
        use super::channel::{ChannelRequestPhase, SessionCommand};
        fn ssh_io_error(_: ()) -> SshError { SshError::Io }
        pub struct ServerSession {
            pub channel: Option<Channel>,
            pub io: PacketIo,
        }
        impl ServerSession {
            // @HANDLER@
            // @WAIT_FOR_CHANNEL@
            // @CHANNEL_CLOSED@
            pub fn io(&mut self) -> &mut PacketIo { &mut self.io }
            // The scripted packets hold no KEXINIT, so every message comes back.
            fn rekey_if_kexinit(&mut self, msg: Vec<u8>) -> Result<Option<Vec<u8>>, SshError> { Ok(Some(msg)) }
            pub fn input_eof(&self) -> bool { self.channel.as_ref().unwrap().eof_received }
            pub fn pty_size(&self) -> (u32, u32) { self.channel.as_ref().unwrap().pty_size }
            pub fn send_capacity(&self) -> usize { self.channel.as_ref().unwrap().send_window as usize }
            pub fn send_data(&mut self, data: &[u8]) -> Result<(), SshError> {
                self.channel.as_mut().unwrap().send_window -= data.len() as u32;
                self.io.send_packet(data).map_err(|_| SshError::Io)
            }
            pub fn recv_data(&mut self) -> Result<Option<Vec<u8>>, SshError> {
                let packet = self.io.input.pop_front().unwrap();
                self.handle_channel_packet(&packet, true)
            }
            pub fn try_recv_data_uncredited(&mut self) -> Result<Option<Vec<u8>>, SshError> {
                self.io.input.pop_front().map_or(Ok(None), |packet| self.handle_channel_packet(&packet, false))
            }
            pub fn consume_input(&mut self, count: usize) -> Result<(), SshError> {
                channel::send_window_adjust(&mut self.io, self.channel.as_mut().unwrap(), count as u32)
            }
        }
    }
}
// @PTY_SIZE@
// @SHUTTLE@
// @CHANNEL_STARTS@

fn request(kind: u8, name: &[u8], want_reply: bool) -> Vec<u8> {
    let mut packet = vec![kind];
    if kind == SSH_MSG_CHANNEL_REQUEST { SshBuf::put_u32(&mut packet, 0); }
    SshBuf::put_string(&mut packet, name);
    SshBuf::put_bool(&mut packet, want_reply);
    packet
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("channel-starts") {
        check_channel_starts();
        return;
    }
    let mut data = vec![SSH_MSG_CHANNEL_DATA];
    SshBuf::put_u32(&mut data, 0);
    SshBuf::put_string(&mut data, b"abc");
    let mut credit = vec![SSH_MSG_CHANNEL_WINDOW_ADJUST];
    SshBuf::put_u32(&mut credit, 0);
    SshBuf::put_u32(&mut credit, 8);
    let mut resize = request(SSH_MSG_CHANNEL_REQUEST, b"window-change", false);
    for n in [104, 36, 0, 0] { SshBuf::put_u32(&mut resize, n); }
    let mut session = ServerSession {
        channel: Some(Channel {
            local_id: 0, remote_id: 7, recv_window: 16, send_window: 0, eof_received: false,
            closed: false, pty_term: Some(String::from("xterm")), pty_size: (80, 24),
        }),
        io: PacketIo { input: [
            data,
            request(SSH_MSG_GLOBAL_REQUEST, b"keepalive@openssh.com", true),
            request(SSH_MSG_CHANNEL_REQUEST, b"keepalive@openssh.com", true),
            request(SSH_MSG_GLOBAL_REQUEST, b"no-reply", false),
            credit,
            resize,
        ].into(), sent: Vec::new() },
    };
    let _ = data_shuttle(&mut session, MASTER, MASTER, true);
    assert_eq!(STEP.with_borrow(|n| *n), 9);
    let mut reply = vec![SSH_MSG_CHANNEL_FAILURE];
    SshBuf::put_u32(&mut reply, 7);
    assert_eq!(session.io.sent[..2], [vec![SSH_MSG_REQUEST_FAILURE], reply]);
    assert_eq!(session.io.sent.iter().filter(|packet| packet[0] == SSH_MSG_REQUEST_FAILURE).count(), 1);
    assert!(session.io.sent.contains(&b"x".to_vec()), "window adjustment did not resume output");
    assert_eq!(session.channel.unwrap().recv_window, 16, "return credit after delivery");
    assert_eq!(WRITES.with_borrow(Clone::clone), [b"ab".to_vec(), b"c".to_vec()]);
    assert_eq!(SIZES.with_borrow(Clone::clone), [(104, 36)]);
}
