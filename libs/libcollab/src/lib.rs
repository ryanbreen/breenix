//! Breenix Collaboration Protocol (BCP) library
//!
//! Provides real-time multi-user session support over TCP.
//! Applications integrate by adding collaboration FDs to their poll set
//! and processing events in their existing event loop.
//!
//! # Architecture
//!
//! - Star topology: host relays messages to all connected clients
//! - Operation-based sync: small draw ops instead of pixel diffs
//! - Binary TLV wire format with 8-byte headers
//! - Last-write-wins conflict resolution (natural for paint apps)
//!
//! # Usage
//!
//! This example uses Breenix TCP syscalls; host doctests compile it only.
//!
//! ```rust,no_run
//! use libcollab::CollabSession;
//! use libbreenix::{io::{poll, PollFd}, socket::SockAddrIn};
//!
//! // Host a session, or pass --join to connect to an existing host.
//! let mut session = if std::env::args().any(|arg| arg == "--join") {
//!     let addr = SockAddrIn::new([127, 0, 0, 1], 7890);
//!     CollabSession::join(&addr, b"Bob").unwrap()
//! } else {
//!     CollabSession::host(7890, b"Alice", 640, 480).unwrap()
//! };
//!
//! // In your poll loop:
//! let mut extra_fds = [PollFd::default(); 16];
//! let n = session.poll_fds(&mut extra_fds);
//! poll(&mut extra_fds[..n], 0).unwrap();
//! session.process_io(&extra_fds[..n]);
//! while let Some(event) = session.next_event() {
//!     println!("{event:?}");
//! }
//! ```

mod wire;
mod peer;
mod event;
mod host;
mod client;
mod session;

pub use event::{CollabEvent, DrawOp};
pub use peer::PeerInfo;
pub use session::CollabSession;
pub use wire::MessageType;
