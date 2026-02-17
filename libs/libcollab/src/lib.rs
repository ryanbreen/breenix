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
//! ```rust,ignore
//! use libcollab::{CollabSession, CollabEvent, DrawOp};
//!
//! // Host a session
//! let mut session = CollabSession::host(7890, b"Alice").unwrap();
//!
//! // Or join one
//! let mut session = CollabSession::join(&addr, b"Bob").unwrap();
//!
//! // In your poll loop:
//! let n = session.poll_fds(&mut extra_fds);
//! // ... add extra_fds to your poll set ...
//! session.process_io(&poll_results)?;
//! while let Some(event) = session.next_event() {
//!     match event { /* handle events */ }
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
