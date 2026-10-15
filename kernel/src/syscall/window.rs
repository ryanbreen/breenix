//! System call 414, `window(op, arg, len)`: the window interface, version 2
//! (`docs/gui/breengel.md` section 17.1).
//!
//! The op table answers `VERSION` (2) and refuses an unknown op or a wrong argument size
//! with EINVAL; the operations themselves are not in this kernel yet and answer ENOSYS.

use breengel_abi::errno::{EINVAL, ENOSYS};
use breengel_abi::{arg_size, op, INTERFACE_VERSION};

use super::SyscallResult;

/// `window(op, arg, len)`.
pub fn sys_window(op: u64, arg: u64, len: u64) -> SyscallResult {
    match window(op, arg, len) {
        Ok(value) => SyscallResult::Ok(value),
        Err(errno) => SyscallResult::Err(errno as u64),
    }
}

fn window(op: u64, arg: u64, len: u64) -> Result<u64, i32> {
    let op = u32::try_from(op).map_err(|_| EINVAL)?;
    match arg_size(op) {
        Some(size) if len != size as u64 => return Err(EINVAL),
        Some(_) => {}
        None if op > op::LAST => return Err(EINVAL),
        None if arg != 0 || len != 0 => return Err(EINVAL),
        None => {}
    }
    if op == op::VERSION {
        return Ok(INTERFACE_VERSION as u64);
    }
    Err(ENOSYS)
}
