//! Socket subsystem for Breenix
//!
//! Provides socket management and networking infrastructure.

pub mod addr;
pub mod object;
pub mod opts;
pub mod tcp;
pub mod types;
pub mod udp;
pub mod unix;
pub mod unix_msg;
pub mod unix_ns;
pub mod wait;

pub use udp::{udp_ports_lookup_refused, SOCKET_REGISTRY};

// Unix names are reserved by the Unix protocol at bind, independently of listen.
pub use unix_ns::UNIX_SOCKET_REGISTRY;
