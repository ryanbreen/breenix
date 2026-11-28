use std::cell::Cell;

thread_local! {
    static CPU: Cell<usize> = const { Cell::new(1) };
    static SPLITS: Cell<usize> = const { Cell::new(0) };
}

struct CpuId(usize);

impl CpuId {
    fn current() -> Self {
        Self(CPU.get())
    }

    #[allow(dead_code)] // Used by the pre-fix entry and by the mutation check.
    fn current_checked(carried: usize) -> Self {
        let fresh = Self::current();
        if carried != fresh.0 {
            SPLITS.set(SPLITS.get() + 1);
        }
        fresh
    }

    fn index(&self) -> usize {
        self.0
    }
}

struct Dispatch(usize);

impl Dispatch {
    fn cpu(&self) -> usize {
        self.0
    }

    fn try_local(cpu: usize) -> Option<Self> {
        assert_eq!(cpu, CPU.get());
        (cpu == 2).then_some(Self(cpu))
    }

    fn available() -> bool {
        true
    }
}

fn retry_with_irqs_enabled() {
    // Model preemption and migration while the retry enables IRQs.
    assert_eq!(CPU.get(), 1);
    CPU.set(2);
}

fn read_daif() {}
fn cpu0_breadcrumb(_: usize, _: u64) {}
fn restore_irqs() {}

mod super_shim {
    pub mod irq_mask_account {
        pub fn closing() {}
    }
}

use super_shim::irq_mask_account;

include!(concat!(env!("OUT_DIR"), "/inline_entry.rs"));

#[test]
fn migration_during_retry_uses_acquired_cpu_without_identity_split() {
    assert_eq!(inline_entry(), Some(2));
    assert_eq!(
        SPLITS.get(),
        0,
        "legal retry migration reported an identity split"
    );
}
