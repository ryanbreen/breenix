//! Per-process Linux resource limits. All updates are serialized by PROCESS_MANAGER.
use super::{Process, ProcessId, ProcessManager};

pub const CPU: usize = 0;
pub const FSIZE: usize = 1;
pub const DATA: usize = 2;
pub const STACK: usize = 3;
pub const CORE: usize = 4;
pub const NPROC: usize = 6;
pub const NOFILE: usize = 7;
pub const AS: usize = 9;
pub const COUNT: usize = 16;
pub const INFINITY: u64 = u64::MAX;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Rlimit {
    pub soft: u64,
    pub hard: u64,
}

pub fn defaults() -> [Rlimit; COUNT] {
    let mut limits = [Rlimit {
        soft: INFINITY,
        hard: INFINITY,
    }; COUNT];
    limits[STACK].soft = 8 << 20;
    limits[NOFILE] = Rlimit {
        soft: crate::ipc::MAX_FDS as u64,
        hard: 4096,
    };
    limits[CORE].soft = 0;
    limits
}

/// Count the user's live tasks under the same lock that publishes a fork.
pub fn fork_allowed(manager: &ProcessManager, pid: ProcessId) -> bool {
    let Some(parent) = manager.get_process(pid) else {
        return false;
    };
    parent.euid == 0
        || (manager
            .iter_processes()
            .filter(|(_, p)| p.uid == parent.uid && !p.is_terminated())
            .count() as u64)
            < parent.limits[NPROC].soft
}

impl Process {
    pub fn mapped_bytes(&self) -> u64 {
        self.image_size
            .saturating_add(self.heap_end.saturating_sub(self.heap_start))
            .saturating_add(self.user_stack_top.saturating_sub(self.user_stack_bottom))
            .saturating_add(self.vmas.iter().map(|v| v.size()).sum::<u64>())
    }

    pub fn data_bytes(&self) -> u64 {
        self.image_data_size
            .saturating_add(self.heap_end.saturating_sub(self.heap_start))
            .saturating_add(
                self.vmas
                    .iter()
                    .filter(|v| {
                        v.flags.contains(crate::memory::vma::MmapFlags::PRIVATE)
                            && v.prot.contains(crate::memory::vma::Protection::WRITE)
                    })
                    .map(|v| v.size())
                    .sum::<u64>(),
            )
    }

    /// Charge CPU time from the scheduler's accumulated and in-flight ticks.
    /// Called at existing signal checks, with a nonblocking scheduler lookup.
    pub fn check_cpu_limit(&mut self) {
        let limit = self.limits[CPU];
        if limit.soft == INFINITY && limit.hard == INFINITY {
            return;
        }
        let Some(ticks) = crate::task::scheduler::process_cpu_ticks(self.id.as_u64()) else {
            return;
        };
        let seconds = ticks.saturating_mul(crate::time::timer::MS_PER_TICK) / 1000;
        if seconds >= limit.hard {
            self.signals.set_pending(crate::signal::constants::SIGKILL);
        } else if seconds >= limit.soft && seconds >= self.cpu_limit_next {
            self.signals.set_pending(crate::signal::constants::SIGXCPU);
            self.cpu_limit_next = seconds.saturating_add(1);
        }
    }
}
