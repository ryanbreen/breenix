//! SCRATCH, never merged: the #1442 forcing, v7.
//!
//! A kernel thread pinned to the CPU that takes device interrupts transmits
//! one frame through e1000 with interrupts enabled, and keeps the driver lock
//! for `HOLD_WINDOW_US` after its tail write. `FACE` picks the trigger:
//!
//! - `CloneVmExec`: the first exec of clonevm_exec_test (its CLONE_VM child's)
//!   routes device interrupts to a CPU other than its own, arms the thread
//!   there, and waits before reading its image until the forcing has been
//!   exercised: on the unfixed driver, that CPU's e1000 interrupt is spinning
//!   on the lock the pinned thread holds; on the fixed driver, the pinned
//!   thread held the lock through the whole window and an e1000 interrupt was
//!   then taken on that CPU. An attempt that is not exercised within
//!   `EXEC_WAIT_US` fails the exec with EAGAIN, so the boot cannot pass on it.
//! - `LoadChild`: loopback_wake_test's load child writing its one-byte "s"
//!   routes device interrupts to the CPU it runs on and arms the thread there.
//!
//! On the unfixed driver the interrupt takes the lock through `irq_lock`,
//! which, once it has waited `STUCK_DUMP_AFTER_US`, writes lock-free state
//! dumps to COM1: the holder, each CPU's tick credit and current thread, the
//! scheduler, process-manager and serial locks, and the load child.
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

#[derive(PartialEq, Eq)]
enum Face {
    CloneVmExec,
    LoadChild,
}
const FACE: Face = Face::LoadChild;

const NO_CPU: u64 = u64::MAX;
static TARGET_CPU: AtomicU64 = AtomicU64::new(NO_CPU);
static STATE: AtomicU8 = AtomicU8::new(IDLE);
const IDLE: u8 = 0;
const ARMED: u8 = 1;
const TX_ENTERED: u8 = 2;
const TX_RETURNED: u8 = 3;

static FORCE_TID: AtomicU64 = AtomicU64::new(u64::MAX);
/// Furthest step the forcing thread reached inside the transmit: 4 driver lock
/// held, 5 tail register written, 7 hold window over, 6 descriptor done.
static STEP: AtomicU8 = AtomicU8::new(0);
static ARMED_US: AtomicU64 = AtomicU64::new(0);
static TX_BEGIN_US: AtomicU64 = AtomicU64::new(0);
static IRQ_ENTRIES: AtomicU64 = AtomicU64::new(0);
/// The forcing thread began its hold window, and finished it, with the driver
/// lock held.
static HOLD_BEGAN: AtomicBool = AtomicBool::new(false);
static HOLD_DONE: AtomicBool = AtomicBool::new(false);
/// e1000 interrupts taken on the target CPU after the hold window began.
static TARGET_IRQS_AFTER_HOLD: AtomicU64 = AtomicU64::new(0);
/// CPU + 1 of an e1000 interrupt that found the driver lock held and has not
/// acquired it yet; 0 when none is waiting.
static IRQ_LOCK_WAIT_CPU: AtomicU64 = AtomicU64::new(0);
static IRQ_LOCK_WAIT_SINCE_US: AtomicU64 = AtomicU64::new(0);
static LOAD_TID: AtomicU64 = AtomicU64::new(u64::MAX);
static KILLS_AFTER_TX: AtomicU64 = AtomicU64::new(0);
/// Thread holding E1000_DRIVER (u64::MAX when free), the site that took it
/// (1 mac_address, 2 link_up, 3 transmit, 4 receive, 5 can_receive), the CPU
/// it took it on and whether interrupts were enabled when it did.
static HOLDER_TID: AtomicU64 = AtomicU64::new(u64::MAX);
static HOLDER_SITE: AtomicU8 = AtomicU8::new(0);
static HOLDER_CPU: AtomicU64 = AtomicU64::new(NO_CPU);
static HOLDER_IF: AtomicU8 = AtomicU8::new(0);
static DUMPER: AtomicBool = AtomicBool::new(false);
/// The exec that armed the forcing, and how far it got: 1 waiting for the
/// forcing, 2 reading its image, 3 image read returned.
static EXEC_TID: AtomicU64 = AtomicU64::new(u64::MAX);
static EXEC_PHASE: AtomicU8 = AtomicU8::new(0);
/// NMI sampling: a dumper sets SAMPLE_REQ[cpu] and sends that CPU an NMI; the
/// shootdown NMI handler stores the interrupted RIP.
static SAMPLE_REQ: [AtomicBool; 4] = [const { AtomicBool::new(false) }; 4];
static SAMPLE_RIP: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
/// How long the forcing thread keeps the driver lock after its tail write.
const HOLD_WINDOW_US: u64 = 50_000;
const EXEC_WAIT_US: u64 = 30_000_000;
const STUCK_DUMP_AFTER_US: u64 = 200_000;
const STUCK_DUMP_EVERY_US: u64 = 1_000_000;
const STUCK_DUMPS: u32 = 15;

pub fn holder_enter(site: u8) {
    HOLDER_SITE.store(site, Ordering::Relaxed);
    HOLDER_IF.store(x86_64::instructions::interrupts::are_enabled() as u8, Ordering::Relaxed);
    HOLDER_CPU.store(crate::per_cpu::cpu_id() as u64, Ordering::Relaxed);
    HOLDER_TID.store(
        crate::task::scheduler::current_thread_id().unwrap_or(u64::MAX - 1),
        Ordering::Release,
    );
}

pub fn holder_exit() {
    HOLDER_TID.store(u64::MAX, Ordering::Release);
}

fn is_forcing_thread() -> bool {
    crate::task::scheduler::current_thread_id() == Some(FORCE_TID.load(Ordering::Relaxed))
}

/// Called with the driver lock held, right after the tail write: the forcing
/// thread keeps the lock for `HOLD_WINDOW_US`, the device's TX-done interrupt
/// arriving meanwhile.
pub fn hold_window() {
    if !is_forcing_thread() {
        return;
    }
    HOLD_BEGAN.store(true, Ordering::Release);
    let begin = now_us();
    while now_us().saturating_sub(begin) < HOLD_WINDOW_US {
        core::hint::spin_loop();
    }
    STEP.store(7, Ordering::Release);
    HOLD_DONE.store(true, Ordering::Release);
}

pub fn now_us() -> u64 {
    let (seconds, nanos) = crate::time::get_monotonic_time_ns();
    seconds.saturating_mul(1_000_000).saturating_add(nanos / 1_000)
}

fn sleep_ms() {
    crate::task::dispatch_strand_census::sleep_one_interval(1_000_000);
}

pub fn step(value: u8) {
    if is_forcing_thread() {
        STEP.store(value, Ordering::Release);
    }
}

pub fn irq_entered() {
    IRQ_ENTRIES.fetch_add(1, Ordering::Relaxed);
    if HOLD_BEGAN.load(Ordering::Acquire)
        && crate::per_cpu::cpu_id() as u64 == TARGET_CPU.load(Ordering::Acquire)
    {
        TARGET_IRQS_AFTER_HOLD.fetch_add(1, Ordering::Release);
    }
}

/// The unfixed driver's interrupt-side acquisition of E1000_DRIVER.
pub fn irq_lock<T>(lock: &spin::Mutex<T>) -> spin::MutexGuard<'_, T> {
    if let Some(guard) = lock.try_lock() {
        return guard;
    }
    let began = now_us();
    IRQ_LOCK_WAIT_SINCE_US.store(began, Ordering::Relaxed);
    IRQ_LOCK_WAIT_CPU.store(crate::per_cpu::cpu_id() as u64 + 1, Ordering::Release);
    let dumper = !DUMPER.swap(true, Ordering::AcqRel);
    let mut next_dump = began + STUCK_DUMP_AFTER_US;
    let mut dumps = 0;
    let guard = loop {
        if let Some(guard) = lock.try_lock() {
            break guard;
        }
        if dumper && dumps < STUCK_DUMPS {
            let now = now_us();
            if now >= next_dump {
                stuck_dump(dumps, now.saturating_sub(began));
                dumps += 1;
                next_dump = now + STUCK_DUMP_EVERY_US;
            }
        }
        core::hint::spin_loop();
    };
    IRQ_LOCK_WAIT_CPU.store(0, Ordering::Release);
    if dumper {
        DUMPER.store(false, Ordering::Release);
    }
    guard
}

/// Called from the shootdown NMI handler on `cpu`, before its own work.
pub fn nmi_sample(cpu: usize, rip: u64) {
    if cpu < SAMPLE_REQ.len() && SAMPLE_REQ[cpu].load(Ordering::Acquire) {
        SAMPLE_RIP[cpu].store(rip, Ordering::Relaxed);
        SAMPLE_REQ[cpu].store(false, Ordering::Release);
    }
}

pub fn exec_image_read_returned() {
    if crate::task::scheduler::current_thread_id() == Some(EXEC_TID.load(Ordering::Relaxed)) {
        EXEC_PHASE.store(3, Ordering::Release);
        crate::serial_println!("[FORCE1442:exec_image_read_returned]");
    }
}

/// NMI every other online CPU and collect its interrupted RIP; 0 when a CPU
/// did not answer within 20 ms.
fn sample_rips(cpus: usize, me: usize) -> [u64; 4] {
    for cpu in 0..cpus {
        SAMPLE_RIP[cpu].store(0, Ordering::Relaxed);
        if cpu == me {
            continue;
        }
        SAMPLE_REQ[cpu].store(true, Ordering::Release);
        if let Some(apic_id) = crate::arch_impl::x86_64::smp::cpu_apic_id(cpu) {
            let _ = crate::arch_impl::x86_64::apic::send_ipi(apic_id, crate::arch_impl::x86_64::apic::Ipi::Nmi);
        }
    }
    let begin = now_us();
    while now_us().saturating_sub(begin) < 20_000
        && (0..cpus).any(|cpu| cpu != me && SAMPLE_REQ[cpu].load(Ordering::Acquire))
    {
        core::hint::spin_loop();
    }
    let mut rips = [0u64; 4];
    for cpu in 0..cpus {
        SAMPLE_REQ[cpu].store(false, Ordering::Release);
        rips[cpu] = SAMPLE_RIP[cpu].load(Ordering::Relaxed);
    }
    rips
}

fn stuck_dump(n: u32, waited_us: u64) {
    let cpus = (crate::arch_impl::x86_64::smp::cpus_online() as usize).min(4);
    let me = crate::per_cpu::cpu_id();
    let rips = sample_rips(cpus, me);
    let mut ticks = [0u64; 4];
    for (cpu, tick) in ticks.iter_mut().enumerate().take(cpus) {
        *tick = crate::arch_impl::x86_64::smp::credited_to_tick(cpu);
    }
    let load_tid = LOAD_TID.load(Ordering::Relaxed);
    let exec_tid = EXEC_TID.load(Ordering::Relaxed);
    let sched = crate::task::scheduler::force1442_try_facts(load_tid);
    let (pm_cpu, pm_tid) = crate::process::force1442_pm_owner();
    let _ = crate::serial::emergency_print(format_args!(
        "\n[FORCE1442:stuck:n={}:irq_cpu={}:waited_us={}:state={}:step={}:force_tid={}:holder_tid={}:holder_site={}:holder_cpu={}:holder_if={}:ticks={:?}:global_tick={}:sched={:?}:load_tid={}:pm_locked={}:pm_owner_cpu={}:pm_owner_tid={}:serial_locked={}:kills_after_tx={}:exec_tid={}:exec_phase={}:rips={:x?}]\n",
        n,
        crate::per_cpu::cpu_id(),
        waited_us,
        STATE.load(Ordering::Acquire),
        STEP.load(Ordering::Acquire),
        FORCE_TID.load(Ordering::Relaxed) as i64,
        HOLDER_TID.load(Ordering::Acquire) as i64,
        HOLDER_SITE.load(Ordering::Relaxed),
        HOLDER_CPU.load(Ordering::Relaxed) as i64,
        HOLDER_IF.load(Ordering::Relaxed),
        &ticks[..cpus],
        crate::time::get_ticks(),
        sched,
        load_tid as i64,
        crate::process::PROCESS_MANAGER.is_locked(),
        pm_cpu as i64,
        pm_tid as i64,
        crate::serial::SERIAL1.is_locked(),
        KILLS_AFTER_TX.load(Ordering::Relaxed),
        exec_tid as i64,
        EXEC_PHASE.load(Ordering::Acquire),
        &rips[..cpus],
    ));
    let tids = [load_tid, exec_tid, FORCE_TID.load(Ordering::Relaxed)];
    if !crate::task::scheduler::force1442_try_dump(n, &tids) {
        let _ = crate::serial::emergency_print(format_args!("[FORCE1442:sched_held:n={}]\n", n));
    }
}

pub fn start() {
    for cpu in 0..crate::arch_impl::x86_64::smp::cpus_online() as usize {
        if crate::task::kthread::kthread_run_on_cpu(move || run(cpu), "force1442", cpu).is_err() {
            crate::serial_println!("[FORCE1442:spawn_failed:cpu={}]", cpu);
        }
    }
    if FACE == Face::LoadChild {
        for cpu in [1usize, 2] {
            if crate::task::kthread::kthread_run_on_cpu(move || watch(cpu), "watch1442", cpu).is_err() {
                crate::serial_println!("[FORCE1442:watch_spawn_failed:cpu={}]", cpu);
            }
        }
    }
}

/// Sample the load child's scheduler state 30 times, 100 ms apart, once the
/// forcing transmit has begun.
fn watch(cpu: usize) {
    while STATE.load(Ordering::Acquire) < TX_ENTERED {
        for _ in 0..10 {
            sleep_ms();
        }
    }
    let tid = LOAD_TID.load(Ordering::Relaxed);
    for sample in 0..30 {
        let facts = crate::task::scheduler::force1442_thread_facts(tid);
        crate::serial_println!(
            "[FORCE1442:watch:cpu={}:n={}:since_tx_begin_us={}:load_tid={}:load_facts={:?}:state={}:step={}:holder_tid={}:irq_lock_wait_cpu={}]",
            cpu,
            sample,
            now_us().saturating_sub(TX_BEGIN_US.load(Ordering::Relaxed)),
            tid,
            facts,
            STATE.load(Ordering::Acquire),
            STEP.load(Ordering::Acquire),
            HOLDER_TID.load(Ordering::Acquire) as i64,
            IRQ_LOCK_WAIT_CPU.load(Ordering::Acquire) as i64 - 1,
        );
        crate::task::dispatch_strand_census::sleep_one_interval(100_000_000);
    }
}

fn run(cpu: usize) {
    loop {
        let target = TARGET_CPU.load(Ordering::Acquire);
        if target == cpu as u64 {
            break;
        }
        if target != NO_CPU {
            return;
        }
        for _ in 0..10 {
            sleep_ms();
        }
    }
    if let Some(tid) = crate::task::scheduler::current_thread_id() {
        FORCE_TID.store(tid, Ordering::Relaxed);
    }
    // Stay current on this CPU from here through the transmit, interrupts
    // still enabled, so the thread this one preempted stays queued here.
    crate::per_cpu::preempt_disable();
    let interrupts = x86_64::instructions::interrupts::are_enabled();
    let begin = now_us();
    TX_BEGIN_US.store(begin, Ordering::Relaxed);
    crate::serial_println!(
        "[FORCE1442:tx_begin:pinned_cpu={}:on_cpu={}:if={}:since_armed_us={}]",
        cpu,
        crate::per_cpu::cpu_id(),
        interrupts,
        begin.saturating_sub(ARMED_US.load(Ordering::Relaxed))
    );
    STATE.store(TX_ENTERED, Ordering::Release);
    let result = crate::net::arp::request(&[10, 0, 2, 2]);
    crate::per_cpu::preempt_enable();
    let end = now_us();
    STATE.store(TX_RETURNED, Ordering::Release);
    crate::serial_println!(
        "[FORCE1442:tx_end:ok={}:elapsed_us={}:hold_done={}:target_irqs_after_hold={}]",
        result.is_ok(),
        end.saturating_sub(begin),
        HOLD_DONE.load(Ordering::Acquire),
        TARGET_IRQS_AFTER_HOLD.load(Ordering::Acquire)
    );
}

fn report(tag: &str) {
    let now = now_us();
    let begin = TX_BEGIN_US.load(Ordering::Relaxed);
    let wait_cpu = IRQ_LOCK_WAIT_CPU.load(Ordering::Acquire);
    crate::serial_println!(
        "[FORCE1442:{}:state={}:step={}:force_tid={}:holder_tid={}:holder_site={}:holder_cpu={}:holder_if={}:hold_began={}:hold_done={}:target_cpu={}:target_irqs_after_hold={}:irq_entries={}:irq_lock_wait_cpu={}:irq_wait_began_after_tx_begin_us={}:irq_waiting_for_us={}:since_armed_us={}]",
        tag,
        STATE.load(Ordering::Acquire),
        STEP.load(Ordering::Acquire),
        FORCE_TID.load(Ordering::Relaxed) as i64,
        HOLDER_TID.load(Ordering::Acquire) as i64,
        HOLDER_SITE.load(Ordering::Relaxed),
        HOLDER_CPU.load(Ordering::Relaxed) as i64,
        HOLDER_IF.load(Ordering::Relaxed),
        HOLD_BEGAN.load(Ordering::Acquire),
        HOLD_DONE.load(Ordering::Acquire),
        TARGET_CPU.load(Ordering::Acquire) as i64,
        TARGET_IRQS_AFTER_HOLD.load(Ordering::Acquire),
        IRQ_ENTRIES.load(Ordering::Relaxed),
        wait_cpu as i64 - 1,
        if wait_cpu == 0 { 0 } else { IRQ_LOCK_WAIT_SINCE_US.load(Ordering::Relaxed).saturating_sub(begin) },
        if wait_cpu == 0 { 0 } else { now.saturating_sub(IRQ_LOCK_WAIT_SINCE_US.load(Ordering::Relaxed)) },
        now.saturating_sub(ARMED_US.load(Ordering::Relaxed))
    );
}

fn arm(cpu: usize) -> bool {
    if STATE
        .compare_exchange(IDLE, ARMED, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return false;
    }
    ARMED_US.store(now_us(), Ordering::Relaxed);
    TARGET_CPU.store(cpu as u64, Ordering::Release);
    true
}

/// The forcing condition holds: on the unfixed driver the target CPU's e1000
/// interrupt is spinning on the lock the forcing thread holds on that CPU, at
/// any of the driver's sites; on the fixed
/// driver the forcing thread held the lock through its window and the target
/// CPU then took an e1000 interrupt.
fn exercised() -> bool {
    let target = TARGET_CPU.load(Ordering::Acquire);
    let state = STATE.load(Ordering::Acquire);
    let unfixed = state == TX_ENTERED
        && HOLDER_TID.load(Ordering::Acquire) == FORCE_TID.load(Ordering::Relaxed)
        && HOLDER_CPU.load(Ordering::Relaxed) == target
        && IRQ_LOCK_WAIT_CPU.load(Ordering::Acquire) == target.wrapping_add(1);
    let fixed = state == TX_RETURNED
        && HOLD_DONE.load(Ordering::Acquire)
        && TARGET_IRQS_AFTER_HOLD.load(Ordering::Acquire) > 0;
    unfixed || fixed
}

/// Runs before the exec reads its image. Returns false, failing the exec, when
/// the forcing was armed here and was not exercised in time.
pub fn arm_and_wait(program: &str) -> bool {
    if FACE != Face::CloneVmExec || program != "/usr/local/test/bin/clonevm_exec_test" {
        return true;
    }
    let online = crate::arch_impl::x86_64::smp::cpus_online() as usize;
    let here = crate::per_cpu::cpu_id();
    let target = if here == online - 1 { online - 2 } else { online - 1 };
    let Some(apic_id) = crate::arch_impl::x86_64::smp::cpu_apic_id(target) else {
        crate::serial_println!("[FORCE1442:exec_unexercised:no_apic_id:target_cpu={}]", target);
        return false;
    };
    if !arm(target) {
        return true;
    }
    if let Some(tid) = crate::task::scheduler::current_thread_id() {
        EXEC_TID.store(tid, Ordering::Relaxed);
    }
    EXEC_PHASE.store(1, Ordering::Release);
    crate::arch_impl::x86_64::ioapic::set_destination(apic_id);
    crate::serial_println!(
        "[FORCE1442:armed:face=clonevm_exec:exec_cpu={}:exec_tid={}:target_cpu={}:apic_id={}]",
        here,
        EXEC_TID.load(Ordering::Relaxed) as i64,
        target,
        apic_id
    );
    // Poll with interrupts enabled rather than sleep: on the unfixed driver
    // the wedge can stop timer wakes from arriving.
    let begin = ARMED_US.load(Ordering::Relaxed);
    let mut ok = exercised();
    while !ok && now_us().saturating_sub(begin) < EXEC_WAIT_US {
        core::hint::spin_loop();
        ok = exercised();
    }
    if ok {
        EXEC_PHASE.store(2, Ordering::Release);
        report("exec_proceeds");
    } else {
        report("exec_unexercised");
    }
    ok
}

/// A one-byte "s" written by a loopback_wake_test process: the load child
/// releasing its peer, just before its ten-second spin.
pub fn note_spin_release_write(byte: u8) {
    if FACE != Face::LoadChild || byte != b's' {
        return;
    }
    let Some(tid) = crate::task::scheduler::current_thread_id() else {
        return;
    };
    let is_test = {
        let guard = crate::process::manager();
        guard
            .as_ref()
            .and_then(|manager| manager.find_process_by_thread(tid))
            .is_some_and(|(_, process)| process.name.starts_with("loopback_wake_test_child_"))
    };
    if !is_test {
        return;
    }
    let cpu = crate::per_cpu::cpu_id();
    let Some(apic_id) = crate::arch_impl::x86_64::smp::cpu_apic_id(cpu) else {
        return;
    };
    if !arm(cpu) {
        return;
    }
    LOAD_TID.store(tid, Ordering::Relaxed);
    crate::arch_impl::x86_64::ioapic::set_destination(apic_id);
    crate::serial_println!("[FORCE1442:armed:face=load_child:tid={}:cpu={}:apic_id={}]", tid, cpu, apic_id);
}

pub fn note_kill(sig: u32) {
    if FACE == Face::LoadChild && sig == 9 && STATE.load(Ordering::Acquire) >= TX_ENTERED {
        if KILLS_AFTER_TX.fetch_add(1, Ordering::AcqRel) == 0 {
            report("first_sigkill");
        }
    }
}
