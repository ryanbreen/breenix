//! Scratch-only busy-CPU forcing and bounded timestamps; never merge.
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
fn now_ns()->u64 { crate::signal::monotonic_nanos() }
fn cpu()->usize { crate::per_cpu::cpu_id() }
struct Event { kind: AtomicU64, tid: AtomicU64, cpu: AtomicU64, deadline: AtomicU64, at: AtomicU64 }
static EVENTS: [Event; 2048] = [const { Event { kind: AtomicU64::new(0), tid: AtomicU64::new(0), cpu: AtomicU64::new(0), deadline: AtomicU64::new(0), at: AtomicU64::new(0) } }; 2048];
static INDEX: AtomicU64 = AtomicU64::new(0);
static DUMP: AtomicBool = AtomicBool::new(false);
static LONG_SLEEP: [AtomicU64; 4096] = [const { AtomicU64::new(0) }; 4096];
static SEEN: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
static BUSY_UNTIL: AtomicU64 = AtomicU64::new(0);
pub fn busy() -> bool { BUSY_UNTIL.load(Ordering::Relaxed) > now_ns() }
pub fn phase(deadline: u64) {
    if BUSY_UNTIL.load(Ordering::Relaxed) > now_ns() && SEEN[cpu()].swap(deadline, Ordering::Relaxed) != deadline {
        crate::per_cpu::set_quantum(10);
        trace(6, crate::per_cpu::current_thread_id_lock_free().unwrap_or(0), deadline, now_ns());
    }
}
pub fn trace(kind: u64, tid: u64, deadline: u64, at: u64) {
    if !ACTIVE.load(Ordering::Relaxed) { return; }
    let slot = &LONG_SLEEP[tid as usize % LONG_SLEEP.len()];
    if kind == 1 {
        slot.store(if deadline.saturating_sub(at) >= 100_000_000 || busy() { deadline } else { 0 }, Ordering::Relaxed);
    }
    if matches!(kind, 1 | 2 | 5) && slot.load(Ordering::Relaxed) != deadline { return; }
    let i = INDEX.fetch_add(1, Ordering::Relaxed) as usize;
    if let Some(e) = EVENTS.get(i) {
        e.tid.store(tid, Ordering::Relaxed);
        e.cpu.store(cpu() as u64, Ordering::Relaxed);
        e.deadline.store(deadline, Ordering::Relaxed);
        e.at.store(at, Ordering::Relaxed);
        e.kind.store(kind, Ordering::Release);
    }
}
pub fn observe_write(bytes: &[u8]) {
    if bytes.windows(b"SUITE time WAIT sleep/nanosleep-sa-restart ".len()).any(|s| s==b"SUITE time WAIT sleep/nanosleep-sa-restart ") {
        WANTED.store(true, Ordering::Release);
        ACTIVE.store(true, Ordering::Release);
        crate::tracing::providers::irq::IRQ_PROVIDER.enable_all();
        crate::tracing::enable();
    }
    if bytes.windows(b"SUITE time CASE sleep/nanosleep-sa-restart ".len()).any(|s|s==b"SUITE time CASE sleep/nanosleep-sa-restart ") { BUSY_UNTIL.store(0, Ordering::Release); }
    if bytes.windows(b"SUITE time CASE sleep/threads ".len()).any(|s|s==b"SUITE time CASE sleep/threads ") { crate::tracing::disable(); ACTIVE.store(false, Ordering::Release); }
    if bytes.windows(b"SUITE time DONE ".len()).any(|s| s == b"SUITE time DONE ") { DUMP.store(true, Ordering::Release); if let Some(h)=WORKERS.lock().first() {crate::task::kthread::kthread_unpark(h);} }
}
pub fn report() {
    if !DUMP.swap(false, Ordering::AcqRel) { return; }
    log::info!("[SLEEP_RECORDS count={}]", INDEX.load(Ordering::Relaxed));
    for e in &EVENTS {
        let k=e.kind.load(Ordering::Acquire);
        if k==6 || (matches!(k,11|22) && e.tid.load(Ordering::Relaxed)<40) {continue;}
        if k!=0 { log::info!("[SLEEP_TRACE kind={} tid={} cpu={} deadline={} at={}]", k, e.tid.load(Ordering::Relaxed), e.cpu.load(Ordering::Relaxed), e.deadline.load(Ordering::Relaxed), e.at.load(Ordering::Relaxed)); }
    }
    for c in 0..crate::arch_impl::x86_64::smp::cpus_online() as usize {
        let buffers=core::ptr::addr_of!(crate::tracing::TRACE_BUFFERS);
        let b=unsafe { &(*buffers)[c] };
        let mut last=0u64;
        for e in b.iter_events() {
            if e.event_type==crate::tracing::TraceEventType::TIMER_TICK {
                if last!=0 && crate::tracing::timestamp_to_nanos(e.timestamp-last)>20_000_000 {log::info!("[SLEEP_TICK_GAP cpu={} before={} after={}]",c,last,e.timestamp);}
                last=e.timestamp;
            }
        }
    }
}

static WORKERS: spin::Mutex<alloc::vec::Vec<crate::task::kthread::KthreadHandle>> = spin::Mutex::new(alloc::vec::Vec::new());
static WANTED: AtomicBool = AtomicBool::new(false);
static RECEIVER_WAITING: AtomicBool = AtomicBool::new(false);
static SENDER_WAITING: AtomicBool = AtomicBool::new(false);
pub fn init_workers() {
    let reporter=crate::task::kthread::kthread_run(|| {
        crate::task::kthread::kthread_park_if(|| !DUMP.load(Ordering::Acquire));
        report();
    }, "sleep_trace").expect("scratch reporter");
    WORKERS.lock().push(reporter);
    for c in 0..crate::arch_impl::x86_64::smp::cpus_online() as usize {
        let handle = crate::task::kthread::kthread_run_on_cpu(|| {
            crate::task::kthread::kthread_park_if(|| !busy());
            while busy() { core::hint::spin_loop(); }
        }, "sleep_busy", c).expect("scratch busy worker");
        WORKERS.lock().push(handle);
    }
}
pub fn published(requested: u64) {
    if !WANTED.load(Ordering::Acquire) { return; }
    if requested == 2_000_000_000 { RECEIVER_WAITING.store(true, Ordering::Release); }
    if requested <= 10_000_000 { SENDER_WAITING.store(true, Ordering::Release); }
    if RECEIVER_WAITING.load(Ordering::Acquire) && SENDER_WAITING.load(Ordering::Acquire) && WANTED.swap(false, Ordering::AcqRel) {
        BUSY_UNTIL.store(now_ns()+1_000_000_000, Ordering::Release);
        trace(13, crate::per_cpu::current_thread_id_lock_free().unwrap_or(0), requested, now_ns());
        for h in WORKERS.lock().iter().skip(1) { crate::task::kthread::kthread_unpark(h); }
    }
}

static ACTIVE: AtomicBool=AtomicBool::new(false);
static HOLD: [AtomicU64;8]=[const{AtomicU64::new(0)};8];
pub fn lock_wait()->u64 { if ACTIVE.load(Ordering::Relaxed) { now_ns() } else { 0 } }
pub fn lock_held(start:u64) { let now=lock_wait(); HOLD[cpu()].store(now,Ordering::Relaxed); if start!=0 && now.saturating_sub(start)>20_000_000 {trace(20,crate::per_cpu::current_thread_id_lock_free().unwrap_or(0),start,now);} }
pub fn lock_end() { let start=HOLD[cpu()].swap(0,Ordering::Relaxed); let now=lock_wait(); if start!=0 && now.saturating_sub(start)>20_000_000 {trace(21,crate::per_cpu::current_thread_id_lock_free().unwrap_or(0),start,now);} }
