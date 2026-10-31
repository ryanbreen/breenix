//! Address-free allocation observations for native GPU case boundaries.
use super::*;

pub(crate) fn record() -> Result<Vec<u8>, i32> {
    let mut words = [0u64; abi::heap_trace::WORDS];
    words[0] = 1;
    words[1] = crate::memory::heap::free_bytes() as u64;
    words[2] = crate::memory::heap::high_water_bytes() as u64;
    let frames = crate::memory::frame_allocator::heap_footprint();
    words[3..6].copy_from_slice(&frames.map(|n| n as u64));
    let cpu = dma::cpu_heap_footprint();
    words[6..9].copy_from_slice(&cpu.map(|n| n as u64));
    with_core(|c| {
        words[9] = c.contexts.len() as u64;
        words[10] = c.resources.len() as u64;
        words[11] = c.subs.len() as u64;
        words[12] = c.syncs.len() as u64;
        words[13] = c.backlog.len() as u64;
        words[14] = (c.backlog.capacity() * core::mem::size_of::<(u64, Request)>()) as u64;
        words[15] = c.resources.values().map(|r| r.backing.metadata_heap_bytes() as u64).sum();
        words[16] = c.contexts.values().map(|ctx| {
            ctx.inflight.capacity() * core::mem::size_of::<u64>()
                + ctx.private_scratch.capacity() * core::mem::size_of::<PrivateScratch>()
                + ctx.depth.as_ref().map_or(0, |d| d.kept.capacity() * core::mem::size_of::<u64>())
                + ctx.native_warm_scratch.as_ref().map_or(0, |s| s.holds.capacity() * core::mem::size_of::<crate::gk208::resource::Hold>())
        }).sum::<usize>() as u64;
        if let Some(Device::Gk208(device)) = &c.dev {
            words[22..26].copy_from_slice(&device.heap_footprint().map(|n| n as u64));
        }
    });
    let native = crate::gk208::heap_footprint();
    for (index, &value) in native[..5].iter().enumerate() { words[17 + index] = value as u64; }
    words[30] = native[5] as u64;
    words[31] = native[6] as u64;
    words[26] = native[7] as u64;
    let console = crate::console::heap_footprint();
    words[27..29].copy_from_slice(&console.map(|n| n as u64));
    words[29] = crate::memory::heap::HEAP_SIZE as u64;
    // A privileged case-boundary reader opts into bounded latency collection.
    words[32..].copy_from_slice(&crate::gk208::cost::snapshot());
    encode(words)
}
fn encode(words: [u64; abi::heap_trace::WORDS]) -> Result<Vec<u8>, i32> {
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(abi::heap_trace::SIZE).map_err(|_| -12)?;
    for word in words { bytes.extend_from_slice(&word.to_le_bytes()); }
    Ok(bytes)
}
