//! The framebuffer system calls' old module path, kept while the x86 dispatch in
//! `handler.rs` still names it.

pub use super::fb::{sys_fbinfo, sys_fbmmap, sys_get_mouse_pos};
pub use super::fb_draw::sys_fbdraw;
