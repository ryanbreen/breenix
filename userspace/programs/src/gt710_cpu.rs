//! Native case-boundary CPU samples, outside the measured case and its verdict.
use std::{sync::Mutex, time::{Duration, Instant}};
static LAST: Mutex<Option<(Instant, [u64;4])>> = Mutex::new(None);
fn split(raw: &[i64;18]) -> Option<[u64;2]> {
    let time = |at: usize| {
        let sec = u64::try_from(raw[at]).ok()?;
        let us = u64::try_from(raw[at+1]).ok()?;
        if us >= 1_000_000 { return None; }
        sec.checked_mul(1_000_000)?.checked_add(us)
    };
    Some([time(0)?,time(2)?])
}
fn sample() -> Option<[u64;4]> {
    let mut result = [0;4];
    for (index, who) in [0u64,u64::MAX].into_iter().enumerate() {
        let mut raw = [0i64;18];
        // SAFETY: x86 Breenix getrusage (98) writes its 18-long ABI struct
        // into this live, writable buffer. who is SELF or waited-for CHILDREN.
        if unsafe { libbreenix::syscall::raw::syscall2(98, who, raw.as_mut_ptr() as u64) } != 0 { return None; }
        result[index*2..index*2+2].copy_from_slice(&split(&raw)?);
    }
    Some(result)
}
fn report(name: &str, before: [u64;4], after: [u64;4], elapsed: Duration) -> Option<String> {
    if name.is_empty() || elapsed < Duration::from_secs(1) { return None; }
    let [u,s,cu,cs] = [after[0].checked_sub(before[0])?,after[1].checked_sub(before[1])?,
        after[2].checked_sub(before[2])?,after[3].checked_sub(before[3])?];
    Some(format!("GT710_CASE_CPU case={name} sample_ms={} self_user_us={u} self_system_us={s} children_user_us={cu} children_system_us={cs}",elapsed.as_millis()))
}
pub(super) fn prepare() {
    if let Ok(mut last) = LAST.lock() { *last = sample().map(|cpu| (Instant::now(),cpu)); }
}
pub(super) fn boundary(index: usize, name: &str) {
    let now = Instant::now();
    let Some(after) = sample() else {
        println!("GT710_CASE_CPU_UNAVAILABLE case={name}");return;
    };
    if let Ok(mut last) = LAST.lock() {
        if index != 0 {
            if let Some((at,before)) = last.as_ref() {
                if let Some(text) = report(name,*before,after,now.duration_since(*at)) { println!("{text}"); }
            }
        }
        *last = Some((now,after));
    }
}
