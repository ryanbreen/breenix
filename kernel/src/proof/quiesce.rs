use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::task::kthread::KthreadHandle;

use super::rng::{self, AntagonistOp};

const RELEASED: u64 = 0;
const PARKED: u64 = 1;
const STOPPED: u64 = 2;

static CONTROL: AtomicU64 = AtomicU64::new(RELEASED);
static ACTIVE_CPUS: AtomicU64 = AtomicU64::new(0);
static CLEAR_OBSERVED: AtomicU64 = AtomicU64::new(0);
static EXITED_CPUS: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Pen,
    Adversarial,
    Ambient,
}

impl Mode {
    pub fn selected() -> Self {
        match option_env!("BREENIX_COREPROOF_MODE") {
            Some("adversarial") => Self::Adversarial,
            Some("ambient") => Self::Ambient,
            _ => Self::Pen,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Pen => "pen",
            Self::Adversarial => "adversarial",
            Self::Ambient => "ambient",
        }
    }
}

pub struct Controller {
    mask: u64,
    handles: Vec<KthreadHandle>,
}

impl Controller {
    pub fn begin(
        mode: Mode,
        root_seed: u64,
        component: u8,
        driver_cpu: usize,
        victim_tid: u64,
        online_cpus: usize,
    ) -> Self {
        if mode == Mode::Ambient || online_cpus <= 1 {
            return Self {
                mask: 0,
                handles: Vec::new(),
            };
        }

        ACTIVE_CPUS.store(0, Ordering::Release);
        CLEAR_OBSERVED.store(0, Ordering::Release);
        EXITED_CPUS.store(0, Ordering::Release);
        CONTROL.store(PARKED, Ordering::Release);

        let mut mask = 0u64;
        let mut handles = Vec::new();
        for cpu in 0..online_cpus.min(crate::arch_impl::aarch64::smp::MAX_CPUS) {
            if cpu == driver_cpu {
                continue;
            }
            let bit = 1u64 << cpu;
            let worker_mode = mode;
            let result = crate::task::kthread::kthread_run_on_cpu_for_test(
                move || {
                    worker(
                        worker_mode,
                        root_seed,
                        component,
                        cpu,
                        victim_tid,
                        online_cpus,
                    )
                },
                "coreproof-peer",
                cpu,
            );
            if let Ok(handle) = result {
                mask |= bit;
                handles.push(handle);
            }
        }

        wait_for_mask(&ACTIVE_CPUS, mask);
        Self { mask, handles }
    }

    pub fn release_and_reform(&self) {
        if self.mask == 0 {
            return;
        }

        CONTROL.store(RELEASED, Ordering::Release);
        wait_for_mask(&CLEAR_OBSERVED, self.mask);
        while ACTIVE_CPUS.load(Ordering::Acquire) & self.mask != 0 {
            core::hint::spin_loop();
        }

        CLEAR_OBSERVED.fetch_and(!self.mask, Ordering::AcqRel);
        CONTROL.store(PARKED, Ordering::Release);
        wait_for_mask(&ACTIVE_CPUS, self.mask);
    }

    pub fn finish(self) {
        if self.mask != 0 {
            CONTROL.store(STOPPED, Ordering::Release);
            wait_for_mask(&CLEAR_OBSERVED, self.mask);
            wait_for_mask(&EXITED_CPUS, self.mask);
        }
        for handle in self.handles {
            let _ = crate::task::kthread::kthread_join(&handle);
        }
    }
}

fn wait_for_mask(word: &AtomicU64, mask: u64) {
    while word.load(Ordering::Acquire) & mask != mask {
        core::hint::spin_loop();
    }
}

fn adversarial_step(
    root_seed: u64,
    component: u8,
    cpu: usize,
    iteration: u64,
    victim_tid: u64,
    online_cpus: usize,
    placement_exercised: &mut bool,
) {
    let vector = rng::draw(root_seed, component, cpu as u8, iteration);
    match vector.antagonist_op {
        AntagonistOp::Unblock => {
            let _ =
                crate::task::scheduler::with_scheduler(|scheduler| scheduler.unblock(victim_tid));
        }
        AntagonistOp::Placement if !*placement_exercised => {
            // This wrapper publishes through spawn_on_cpu_for_test, so the
            // placement protocol is exercised without exposing a raw Thread.
            let online = online_cpus.min(crate::arch_impl::aarch64::smp::MAX_CPUS);
            let target = usize::from(vector.antagonist_cpu) % online.max(1);
            let _ =
                crate::task::kthread::kthread_run_on_cpu_for_test(|| {}, "coreproof-place", target);
            *placement_exercised = true;
        }
        AntagonistOp::Placement => core::hint::spin_loop(),
    }
}

fn worker(
    mode: Mode,
    root_seed: u64,
    component: u8,
    cpu: usize,
    victim_tid: u64,
    online_cpus: usize,
) {
    let bit = 1u64 << cpu;
    let mut iteration = 0u64;
    let mut placement_exercised = false;

    loop {
        match CONTROL.load(Ordering::Acquire) {
            PARKED => {
                ACTIVE_CPUS.fetch_or(bit, Ordering::AcqRel);
                if mode == Mode::Adversarial {
                    adversarial_step(
                        root_seed,
                        component,
                        cpu,
                        iteration,
                        victim_tid,
                        online_cpus,
                        &mut placement_exercised,
                    );
                    iteration = iteration.wrapping_add(1);
                } else {
                    core::hint::spin_loop();
                }
            }
            RELEASED => {
                ACTIVE_CPUS.fetch_and(!bit, Ordering::AcqRel);
                CLEAR_OBSERVED.fetch_or(bit, Ordering::AcqRel);
                while CONTROL.load(Ordering::Acquire) == RELEASED {
                    core::hint::spin_loop();
                }
            }
            _ => {
                ACTIVE_CPUS.fetch_and(!bit, Ordering::AcqRel);
                CLEAR_OBSERVED.fetch_or(bit, Ordering::AcqRel);
                EXITED_CPUS.fetch_or(bit, Ordering::Release);
                return;
            }
        }
    }
}
