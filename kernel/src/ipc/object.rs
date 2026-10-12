//! Shared descriptor operations for IPC objects.
use crate::syscall::errno::EINVAL;

/// Close hooks may run under PROCESS_MANAGER: object locks only, no wake or signal.
pub trait IpcFile: Send + Sync + core::fmt::Debug {
    fn as_any(&self) -> &dyn core::any::Any;
    fn fill_stat(&self, st: &mut crate::syscall::fs::Stat);
    fn truncate(&self, _len: u64) -> Result<(), u64> {
        Err(EINVAL as u64)
    }
    fn poll_events(&self) -> i16 {
        0
    }
    fn on_close(&self, _pid: crate::process::ProcessId) {}
}

/// IPC registrations belong to the process, including a close by another row
/// of a CLONE_THREAD group. Capture this before exec clears group membership.
pub fn close_owner(
    row: crate::process::ProcessId,
    thread_group_id: Option<u64>,
) -> crate::process::ProcessId {
    crate::process::ProcessId::new(thread_group_id.unwrap_or(row.as_u64()))
}

#[cfg(test)]
mod tests {
    use super::close_owner;
    use crate::process::ProcessId;

    #[test]
    fn close_owner_is_the_group_for_leaders_and_nonleaders() {
        assert_eq!(close_owner(ProcessId::new(20), None).as_u64(), 20);
        assert_eq!(close_owner(ProcessId::new(20), Some(20)).as_u64(), 20);
        assert_eq!(close_owner(ProcessId::new(21), Some(20)).as_u64(), 20);
    }
}
