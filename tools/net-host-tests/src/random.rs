//! The kernel's keyed hash, at the path `tcp_ident` names it by.

#[path = "../../../kernel/src/random/blake2s.rs"]
pub mod blake2s;
#[path = "../../../kernel/src/random/erase.rs"]
mod erase;
