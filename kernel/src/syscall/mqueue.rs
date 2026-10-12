//! Dispatch seam owned by the mqueue lane.
use super::SyscallResult;

pub fn dispatch(_n: u64, _a: [u64; 6]) -> Option<SyscallResult> {
    None
}
