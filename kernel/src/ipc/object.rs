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
