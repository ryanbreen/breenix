//! A health round may feed only after I/O succeeds. Readback detects
//! lost configuration before the next reset command.
pub trait Transport {
    type State;
    fn configured(state: &Self::State) -> bool;
    fn get(&mut self) -> Result<Self::State, &'static str>;
    fn set(&mut self) -> Result<(), &'static str>;
    fn reset(&mut self) -> Result<(), &'static str>;
}
pub fn feed<T: Transport>(transport: &mut T, health: impl FnOnce() -> Result<(), &'static str>) -> Result<bool, &'static str> {
    health()?;
    let rearm = !T::configured(&transport.get()?);
    if rearm { transport.set()?; }
    transport.reset()?;
    if !T::configured(&transport.get()?) { return Err("watchdog settings readback"); }
    Ok(rearm)
}

#[cfg(not(test))]
pub fn wait(ms: u64) -> Result<(), &'static str> {
    use crate::task::{scheduler, thread::ThreadState};
    let tid = scheduler::current_thread_id().ok_or("watchdog has no thread context")?;
    let (seconds, nanos) = crate::time::get_monotonic_time_ns();
    let until = seconds.saturating_mul(1_000_000_000).saturating_add(nanos)
        .saturating_add(ms.saturating_mul(1_000_000));
    scheduler::with_current(|scheduler| scheduler.block_current_for_timer(until));
    scheduler::yield_current();
    loop {
        crate::arch_halt_with_interrupts();
        match scheduler::thread_state(tid) {
            Some(ThreadState::BlockedOnTimer) => {},
            Some(_) => return Ok(()),
            None => return Err("watchdog thread disappeared"),
        }
    }
}
#[cfg(not(test))]
pub fn health() -> Result<(), &'static str> {
    let guard = crate::fs::ext2::root_fs_read()?;
    let fs = guard.as_ref().ok_or("root absent")?;
    let (superblock, groups) = fs.persisted_metadata()?;
    if groups.is_empty() || superblock.block_size() == 0 || !fs.read_inode(crate::fs::ext2::root_inode())?.is_dir() {
        return Err("root metadata invalid");
    }
    Ok(())
}
