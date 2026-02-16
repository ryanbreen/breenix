//! Poll implementation for monitoring file descriptors
//!
//! This module provides the poll() syscall implementation that allows
//! monitoring multiple file descriptors for I/O readiness.

use super::fd::{FdKind, FileDescriptor};

/// Poll event flags (matching Linux definitions)
pub mod events {
    /// Data available to read
    pub const POLLIN: i16 = 0x0001;
    /// Write won't block
    pub const POLLOUT: i16 = 0x0004;
    /// Error condition (output only)
    pub const POLLERR: i16 = 0x0008;
    /// Hang up (output only)
    pub const POLLHUP: i16 = 0x0010;
    /// Invalid fd (output only)
    pub const POLLNVAL: i16 = 0x0020;
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

    match &fd_entry.kind {
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

            // Check for data available
            if (events & events::POLLIN) != 0 {
                if pipe.available() > 0 {
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

            // Check for space available
            if (events & events::POLLOUT) != 0 {
                if pipe.space() > 0 && pipe.has_readers() {
                    revents |= events::POLLOUT;
                }
            }

            // Check for read end closed (error condition for writers)
            if !pipe.has_readers() {
                revents |= events::POLLERR;
            }
        }
        FdKind::FifoRead(_, buffer) => {
            let pipe = buffer.lock();
            if (events & events::POLLIN) != 0 && pipe.available() > 0 {
                revents |= events::POLLIN;
            }
            if !pipe.has_writers() {
                revents |= events::POLLHUP;
            }
        }
        FdKind::FifoWrite(_, buffer) => {
            let pipe = buffer.lock();
            if (events & events::POLLOUT) != 0 && pipe.space() > 0 && pipe.has_readers() {
                revents |= events::POLLOUT;
            }
            if !pipe.has_readers() {
                revents |= events::POLLERR;
            }
        }
        FdKind::UdpSocket(_socket) => {
            // For UDP sockets: we don't implement poll properly yet
            // Just mark as always writable for now
            if (events & events::POLLOUT) != 0 {
                revents |= events::POLLOUT;
            }
            // TODO: Check socket RX queue for POLLIN
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
                    // Console/TTY: always writable, not readable (no input buffer yet)
                    if (events & events::POLLOUT) != 0 {
                        revents |= events::POLLOUT;
                    }
                    // TODO: Check input buffer for POLLIN when implemented
                }
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
        FdKind::TcpSocket(_) => {
            // Unconnected TCP socket - always writable (for connect attempt)
            if (events & events::POLLOUT) != 0 {
                revents |= events::POLLOUT;
            }
        }
        FdKind::TcpListener(port) => {
            // Listening socket - check for pending connections
            if (events & events::POLLIN) != 0 {
                if crate::net::tcp::tcp_has_pending(*port) {
                    revents |= events::POLLIN;
                }
            }
        }
        FdKind::TcpConnection(conn_id) => {
            // Connected socket - check for data and connection state
            let connections = crate::net::tcp::TCP_CONNECTIONS.lock();
            if let Some(conn) = connections.get(conn_id) {
                // Check for readable data
                if (events & events::POLLIN) != 0 {
                    if !conn.rx_buffer.is_empty() {
                        revents |= events::POLLIN;
                    }
                }
                // Check for writable
                if (events & events::POLLOUT) != 0 {
                    if conn.state == crate::net::tcp::TcpState::Established {
                        revents |= events::POLLOUT;
                    }
                }
                // Check for connection closed
                if conn.state == crate::net::tcp::TcpState::Closed ||
                   conn.state == crate::net::tcp::TcpState::CloseWait {
                    revents |= events::POLLHUP;
                }
            } else {
                // Connection not found - error
                revents |= events::POLLERR;
            }
        }
        FdKind::PtyMaster(pty_num) => {
            // PTY master - check slave_to_master buffer for readable data
            if let Some(pair) = crate::tty::pty::get(*pty_num) {
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
            } else {
                revents |= events::POLLERR;
            }
        }
        FdKind::PtySlave(pty_num) => {
            // PTY slave - check line discipline for readable data
            if let Some(pair) = crate::tty::pty::get(*pty_num) {
                if (events & events::POLLIN) != 0 {
                    let ldisc = pair.ldisc.lock();
                    if ldisc.has_data() {
                        revents |= events::POLLIN;
                    }
                }
                if (events & events::POLLOUT) != 0 {
                    let buffer = pair.slave_to_master.lock();
                    // Can write if buffer not full
                    if buffer.available() < 4096 {
                        revents |= events::POLLOUT;
                    }
                }
            } else {
                revents |= events::POLLERR;
            }
        }
        FdKind::UnixStream(socket_ref) => {
            let socket = socket_ref.lock();
            // Check for readable data
            if (events & events::POLLIN) != 0 {
                if socket.has_data() {
                    revents |= events::POLLIN;
                }
            }
            // Check for writable
            if (events & events::POLLOUT) != 0 {
                if !socket.peer_closed() {
                    revents |= events::POLLOUT;
                }
            }
            // Check for peer closed
            if socket.peer_closed() && !socket.has_data() {
                revents |= events::POLLHUP;
            }
        }
        FdKind::UnixSocket(_) => {
            // Unconnected Unix socket - always writable (for connect attempt)
            if (events & events::POLLOUT) != 0 {
                revents |= events::POLLOUT;
            }
        }
        FdKind::UnixListener(listener_ref) => {
            // Listening socket - check for pending connections
            if (events & events::POLLIN) != 0 {
                let listener = listener_ref.lock();
                if listener.has_pending() {
                    revents |= events::POLLIN;
                }
            }
        }
        FdKind::ProcfsFile { content, position } => {
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
