//! Failure-only ARM virtio read receipt (E24 ledger 520(r), #3065).
use core::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VirtioReadFailure {
    pub reason: &'static str,
    pub stage: &'static str,
    pub device: usize,
    pub sector: u64,
    pub bytes: usize,
    pub thread: Option<u64>,
    pub preempt_count: u32,
    pub kill_custody: Option<u64>,
    pub in_interrupt: bool,
    pub interrupts_enabled: bool,
    pub timer_ready: bool,
    pub gate_locked: bool,
    pub gate_wedged: bool,
    pub pending_token: u32,
    pub completed_token: u32,
    pub waiter_thread: u64,
    pub completed_desc: u32,
    pub completed_status: u32,
    pub used_idx: u32,
    pub last_used_idx: u32,
    pub device_status: u32,
}
impl VirtioReadFailure {
    /// These branches return before acquiring or issuing this read request.
    pub fn context_refused(self) -> bool {
        matches!(self.stage, "gate-cannot-sleep" | "gate-publish-refused" | "gate-kill-claimed")
    }
}
impl fmt::Display for VirtioReadFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "virtio_reason={:?} virtio_stage={} virtio_device={} virtio_sector={} virtio_bytes={} virtio_thread={:?} virtio_preempt_count={} virtio_kill_custody={:?} virtio_in_interrupt={} virtio_irqs={} virtio_timer={} virtio_gate_locked={} virtio_gate_wedged={} virtio_pending_token={} virtio_completed_token={} virtio_waiter_thread={} virtio_completed_desc={} virtio_completed_status={} virtio_used_idx={} virtio_last_used_idx={} virtio_device_status={:#x}",
            self.reason, self.stage, self.device, self.sector, self.bytes, self.thread,
            self.preempt_count, self.kill_custody, u8::from(self.in_interrupt), u8::from(self.interrupts_enabled),
            u8::from(self.timer_ready), u8::from(self.gate_locked), u8::from(self.gate_wedged),
            self.pending_token, self.completed_token, self.waiter_thread, self.completed_desc, self.completed_status,
            self.used_idx, self.last_used_idx, self.device_status)
    }
}
