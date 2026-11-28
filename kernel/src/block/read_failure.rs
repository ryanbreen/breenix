//! Typed block read failures (E24 coordinator ledger 520(r) and 734(i1)).
use core::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AhciReadError {
    Range,
    BufferSize,
    /// The caller's context may neither park nor poll (design §6): a
    /// receipt, and EIO.
    NotSleepable,
    /// Early boot polls its own commands, so a port another thread owns
    /// there is a contract violation, not a wait.
    EarlyBootPortOwned,
    /// No completion source will complete a sleeping caller's command on
    /// this CPU: the controller has no interrupt and this CPU no poller.
    CompletionNotReady,
    Interrupted,
    WaiterAllocation,
    WaitPublication,
    AdmissionTimeout,
    ControllerAbsent,
    Setup(&'static str),
    Issue(&'static str),
    Wait(&'static str),
    Finish(&'static str),
}

impl AhciReadError {
    pub fn phase(self) -> &'static str {
        match self {
            Self::ControllerAbsent | Self::Setup(_) => "setup",
            Self::Issue(_) => "issue",
            Self::Wait(_) => "wait",
            Self::Finish(_) => "finish",
            _ => "admission",
        }
    }

    /// A failure before submission cannot establish a failure of the sector.
    pub fn pre_issue(self) -> bool {
        !matches!(self, Self::Wait(_) | Self::Finish(_))
    }
}

impl fmt::Display for AhciReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ahci_phase={} ahci_error={self:?}", self.phase())
    }
}

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
    /// A kill refused the request before it was acquired or issued.
    pub fn context_refused(self) -> bool {
        matches!(self.stage, "gate-publish-refused" | "gate-kill-claimed")
    }

    /// The request was refused before it was issued, so it establishes no
    /// failure of the sector: a kill, or the block I/O caller rule (design
    /// §6), which refuses a context that may not sleep with EIO, or a device
    /// with no armed completion source (`irq-unavailable`), which
    /// `read_sectors` refuses before `transfer` issues anything.
    pub fn pre_issue(self) -> bool {
        self.context_refused()
            || matches!(self.stage, "gate-not-sleepable" | "gate-early-boot-busy" | "irq-unavailable")
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
