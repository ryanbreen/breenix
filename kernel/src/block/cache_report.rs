//! Text representation of a buffer-cache counter snapshot.
use super::cache::Counters;
use alloc::{format, string::String};

pub fn render(counters: &Counters) -> String {
    format!(
        "hits {} {}\nmisses {} {}\ndevice_reads {} {}\ndevice_writes {} {}\nflushes {} {}\n",
        counters.hits.count, counters.hits.sequence,
        counters.misses.count, counters.misses.sequence,
        counters.device_reads.count, counters.device_reads.sequence,
        counters.device_writes.count, counters.device_writes.sequence,
        counters.flushes.count, counters.flushes.sequence,
    )
}
