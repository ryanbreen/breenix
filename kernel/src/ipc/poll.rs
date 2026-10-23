//! Poll implementation for monitoring file descriptors
//!
//! This module provides the poll() syscall implementation that allows
//! monitoring multiple file descriptors for I/O readiness.

use super::fd::{FdKind, FileDescriptor};

/// Poll event flags (matching Linux definitions)
pub mod events {
    /// Data available to read
    pub const POLLIN: i16 = 0x0001;
    /// Priority data available
    pub const POLLPRI: i16 = 0x0002;
    /// Write won't block
    pub const POLLOUT: i16 = 0x0004;
    /// Error condition (output only)
    pub const POLLERR: i16 = 0x0008;
    /// Hang up (output only)
    pub const POLLHUP: i16 = 0x0010;
    /// Invalid fd (output only)
    pub const POLLNVAL: i16 = 0x0020;
    /// The peer shut down its writing side (Linux)
    pub const POLLRDHUP: i16 = 0x2000;
}

/// pollfd structure matching Linux definition
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PollFd {
    /// File descriptor to poll
    pub fd: i32,
    /// Events to poll for (input)
    pub events: i16,
    /// Events that occurred (output)
    pub revents: i16,
}

/// Poll a single file descriptor for readiness
///
/// Returns the revents for this fd based on requested events
pub fn poll_fd(fd_entry: &FileDescriptor, events: i16) -> i16 {
    let mut revents: i16 = 0;

    // A terminal revoked from the description (`tty::revoke`) is hung up for
    // it: readable (end of file) and writable (EIO), with POLLERR and POLLHUP.
    if fd_entry.terminal_revoked() {
        return (events & (events::POLLIN | events::POLLOUT)) | events::POLLERR | events::POLLHUP;
    }

    match &fd_entry.kind {
        FdKind::Ipc(object) => {
            revents = object.poll_events() & (events | events::POLLERR | events::POLLHUP);
        }
        FdKind::StdIo(n) => {
            match *n {
                0 => {
                    // stdin - check if data available
                    if (events & events::POLLIN) != 0 {
                        if super::stdin::has_data() {
                            revents |= events::POLLIN;
                        }
                    }
                }
                1 | 2 => {
                    // stdout/stderr - always writable
                    if (events & events::POLLOUT) != 0 {
                        revents |= events::POLLOUT;
                    }
                }
                _ => {
                    // Unknown stdio fd
                }
            }
        }
        FdKind::PipeRead(buffer) => {
            let pipe = buffer.lock();

            // A read that returns data or EOF does not block.
            if (events & events::POLLIN) != 0 {
                if pipe.available() > 0 || !pipe.has_writers() {
                    revents |= events::POLLIN;
                }
            }

            // Check for write end closed (HUP)
            if !pipe.has_writers() {
                revents |= events::POLLHUP;
            }
        }
        FdKind::PipeWrite(buffer) => {
            let pipe = buffer.lock();

            // A write with no readers returns EPIPE without blocking.
            if (events & events::POLLOUT) != 0 {
                if pipe.space() >= super::pipe::PIPE_BUF || !pipe.has_readers() {
                    revents |= events::POLLOUT;
                }
            }

            // Check for read end closed (error condition for writers)
            if !pipe.has_readers() {
                revents |= events::POLLERR;
            }
        }
        FdKind::FifoRead(_, buffer, _) => {
            let pipe = buffer.lock();
            if (events & events::POLLIN) != 0 && pipe.available() > 0 {
                revents |= events::POLLIN;
            }
            if pipe.has_hung_up() {
                revents |= events::POLLHUP;
            }
        }
        FdKind::FifoReadWrite(_, buffer, _) => {
            let pipe = buffer.lock();
            if events & events::POLLIN != 0 && pipe.available() > 0 { revents |= events::POLLIN; }
            if events & events::POLLOUT != 0 && pipe.space() >= super::pipe::PIPE_BUF { revents |= events::POLLOUT; }
        }
        FdKind::FifoWrite(_, buffer, _) => {
            let pipe = buffer.lock();
            if (events & events::POLLOUT) != 0
                && (pipe.space() >= super::pipe::PIPE_BUF || !pipe.has_readers())
            {
                revents |= events::POLLOUT;
            }
            if !pipe.has_readers() {
                revents |= events::POLLERR;
            }
        }
        FdKind::Socket(socket) => {
            // The protocol reports its readiness, and a pending SO_ERROR is
            // POLLERR on every protocol (`Socket::poll`). POLLERR and POLLHUP
            // are reported whether or not they were asked for.
            let bits = socket.poll() as i16;
            revents = bits & (events | events::POLLERR | events::POLLHUP);
        }
        FdKind::RegularFile(_file) => {
            // Regular files are always readable/writable (for now)
            if (events & events::POLLIN) != 0 {
                revents |= events::POLLIN;
            }
            if (events & events::POLLOUT) != 0 {
                revents |= events::POLLOUT;
            }
        }
        FdKind::Directory(_dir) => {
            // Directories are always "readable" for getdents purposes
            if (events & events::POLLIN) != 0 {
                revents |= events::POLLIN;
            }
        }
        FdKind::Device(device_type) => {
            // Device files have different poll behavior based on type
            use crate::fs::devfs::DeviceType;
            match device_type {
                DeviceType::Kmsg => { revents |= events & events::POLLOUT; }
                DeviceType::Rtc => {} // Only RTC_RD_TIME is implemented.
                DeviceType::Ptmx => {} // Never an open descriptor: open gives a master.
                DeviceType::Null => {
                    // /dev/null: always readable (returns EOF), always writable
                    if (events & events::POLLIN) != 0 {
                        revents |= events::POLLIN;
                    }
                    if (events & events::POLLOUT) != 0 {
                        revents |= events::POLLOUT;
                    }
                }
                DeviceType::Zero => {
                    // /dev/zero: always readable (infinite zeros), always writable
                    if (events & events::POLLIN) != 0 {
                        revents |= events::POLLIN;
                    }
                    if (events & events::POLLOUT) != 0 {
                        revents |= events::POLLOUT;
                    }
                }
                DeviceType::Console | DeviceType::Tty => {
                    // Console/Tty readiness follows the same ring as read().
                    if (events & events::POLLOUT) != 0 {
                        revents |= events::POLLOUT;
                    }
                    if (events & events::POLLIN) != 0 && crate::ipc::stdin::has_data() {
                        revents |= events::POLLIN;
                    }
                }
                DeviceType::Tty0 | DeviceType::Vt(_) => {
                    // A virtual terminal's readiness follows its own input queue.
                    let tty = device_type.terminal().and_then(crate::tty::driver::vt);
                    if (events & events::POLLOUT) != 0 {
                        revents |= events::POLLOUT;
                    }
                    if (events & events::POLLIN) != 0 && tty.is_some_and(|tty| tty.input_ready()) {
                        revents |= events::POLLIN;
                    }
                }
                // The screens are read through their own descriptors.
                DeviceType::Vcs(_) | DeviceType::Vcsa(_) => {}
            }
        }
        FdKind::DevfsDirectory { .. } => {
            // Devfs directory is always "readable" for getdents purposes
            if (events & events::POLLIN) != 0 {
                revents |= events::POLLIN;
            }
        }
        FdKind::DevptsDirectory { .. } => {
            // Devpts directory is always "readable" for getdents purposes
            if (events & events::POLLIN) != 0 {
                revents |= events::POLLIN;
            }
        }
        FdKind::PtyMaster(pair) => {
            // PTY master - check slave_to_master buffer for readable data
            if (events & events::POLLIN) != 0 {
                let buffer = pair.slave_to_master.lock();
                if !buffer.is_empty() {
                    revents |= events::POLLIN;
                }
            }
            if (events & events::POLLOUT) != 0 {
                // Master can always write (goes through line discipline)
                revents |= events::POLLOUT;
            }
            // POLLHUP only when slave was opened and then all slave FDs closed.
            // If slave was never opened (child hasn't connected yet), no hangup.
            if pair.has_slave_hung_up() {
                revents |= events::POLLHUP;
            }
        }
        FdKind::PtySlave(pair) => {
            if pair.master_closed() {
                // A hung-up terminal is readable (end of file) and writable
                // (EIO), with POLLERR and POLLHUP, as Linux reports it.
                revents |= (events & (events::POLLIN | events::POLLOUT))
                    | events::POLLERR
                    | events::POLLHUP;
            } else {
                if (events & events::POLLIN) != 0 {
                    let ldisc = pair.ldisc.lock();
                    if ldisc.has_data() {
                        revents |= events::POLLIN;
                    }
                }
                // Writable while output runs and the ring has room.
                if (events & events::POLLOUT) != 0 && !pair.output_stopped() {
                    let buffer = pair.slave_to_master.lock();
                    if buffer.space() > 0 {
                        revents |= events::POLLOUT;
                    }
                }
            }
        }
        FdKind::ProcfsFile { content, position, .. } => {
            // Procfs file is readable if there's remaining content
            if (events & events::POLLIN) != 0 && *position < content.len() {
                revents |= events::POLLIN;
            }
        }
        FdKind::ProcfsDirectory { .. } => {
            // Procfs directory is always "readable" for getdents purposes
            if (events & events::POLLIN) != 0 {
                revents |= events::POLLIN;
            }
        }
        FdKind::Epoll(_) => {
            // Epoll fds are not directly pollable
        }
    }

    revents
}

/// Check if a file descriptor is readable (POLLIN)
/// Returns true if there is data available to read
pub fn check_readable(fd_entry: &FileDescriptor) -> bool {
    (poll_fd(fd_entry, events::POLLIN) & events::POLLIN) != 0
}

/// Check if a file descriptor is writable (POLLOUT)
/// Returns true if writing would not block
pub fn check_writable(fd_entry: &FileDescriptor) -> bool {
    (poll_fd(fd_entry, events::POLLOUT) & events::POLLOUT) != 0
}

/// Check if a file descriptor has an exception condition (POLLERR, POLLHUP)
/// Returns true if there is an error or hangup condition
pub fn check_exception(fd_entry: &FileDescriptor) -> bool {
    let revents = poll_fd(fd_entry, events::POLLIN | events::POLLOUT);
    (revents & (events::POLLERR | events::POLLHUP)) != 0
}

/// The receive-side readiness state of a connected TCP fd: the monotonic
/// nanosecond instant at which bytes were most recently published into its
/// receive buffer (`0` before the first publication), and how many bytes are in
/// that buffer now. `None` for anything but a connected TCP socket, and for a
/// connection id that no longer resolves: a listener or an unconnected socket
/// has no receive buffer.
///
/// #693 is the reason this exists. A blocking `poll()` that returns 0 ready fds
/// after its whole timeout is indistinguishable, from outside, between
/// "readiness was published while I was parked and I failed to report it" and
/// "no bytes were published before my deadline" -- and the issue stalled on
/// exactly that fork.
/// These two numbers, read at the instant the poll gives up, decide it from
/// kernel state: see `poll_report_timeout` in `syscall/handlers.rs`.
pub fn tcp_rx_publication(fd_entry: &FileDescriptor) -> Option<(u64, usize)> {
    let FdKind::Socket(socket) = &fd_entry.kind else {
        return None;
    };
    let crate::socket::object::SocketProto::Tcp(tcp) = &socket.proto else {
        return None;
    };
    let conn_id = tcp.connection()?;
    crate::net::tcp::with_tcp_connections(|connections| {
        connections
            .get(&conn_id)
            .map(|conn| (conn.rx_publish_ns(), conn.rx_len()))
    })
}
