/*
 * Copyright 2011 Christoph Bumiller
 * Copyright 2015 Samuel Pitoiset
 *
 * Permission is hereby granted, free of charge, to any person obtaining a
 * copy of this software and associated documentation files (the "Software"),
 * to deal in the Software without restriction, including without limitation
 * the rights to use, copy, modify, merge, publish, distribute, sublicense,
 * and/or sell copies of the Software, and to permit persons to whom the
 * Software is furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in
 * all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
 * THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR
 * OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
 * ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
 * OTHER DEALINGS IN THE SOFTWARE.
 */

/*******************************************************************************
    Copyright (c) 2020, NVIDIA CORPORATION. All rights reserved.

    Permission is hereby granted, free of charge, to any person obtaining a
    copy of this software and associated documentation files (the "Software"),
    to deal in the Software without restriction, including without limitation
    the rights to use, copy, modify, merge, publish, distribute, sublicense,
    and/or sell copies of the Software, and to permit persons to whom the
    Software is furnished to do so, subject to the following conditions:

    The above copyright notice and this permission notice shall be included in
    all copies or substantial portions of the Software.

    THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
    IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
    FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
    THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
    LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
    FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
    DEALINGS IN THE SOFTWARE.

*******************************************************************************/

//! Device-clock observations for native diagnosis.
//! Packet/layout: MIT Mesa nvc0_query_hw.c at 769e51468b49b2a42f0a0eaf71cf9eed5ff4e5de,
//! query_get and timestamp result; MIT NVIDIA cla197.h at
//! 9fdf5c4062007929d9f4e6cbad9c9771fe61b880, REPORT_SEMAPHORE_D.
//! Use PIPELINE_LOCATION_ALL rather than Mesa's streaming-output location.

pub fn sampled(sequence: u32) -> bool { sequence != 0 && sequence % 256 == 0 }
pub fn offset(sequence: u32, stage: usize) -> Result<u64, &'static str> {
    if sequence == 0 || stage >= 3 { return Err("GPU timestamp extent"); }
    Ok(1024 + u64::from((sequence - 1) % 4) * 64 + stage as u64 * 16)
}
pub fn query(base: u64, sequence: u32, stage: usize) -> Result<[u32; 5], &'static str> {
    let address = base.checked_add(offset(sequence, stage)?).filter(|a| *a < 1 << 40)
        .ok_or("GPU timestamp address")?;
    // REPORT_ONLY, PIPELINE_LOCATION_ALL, four-word structure. Its last two
    // words are the nanosecond device timestamp; the first echoes sequence.
    Ok([0x200406c0, (address >> 32) as u32, address as u32, sequence, 0x0000f002])
}
pub fn decode(sequence: u32, words: u32, raw: [[u32; 4]; 3]) -> [u64; 6] {
    let stamps = raw.map(|r| u64::from(r[2]) | u64::from(r[3]) << 32);
    let valid = raw.iter().all(|r| r[0] == sequence) && stamps[0] != 0
        && stamps[0] <= stamps[1] && stamps[1] <= stamps[2];
    [u64::from(sequence), u64::from(words), stamps[0], stamps[1], stamps[2], u64::from(valid)]
}

pub fn end_offset(sequence: u32) -> Result<u64, &'static str> {
    if sequence == 0 { return Err("GPU end timestamp extent"); }
    Ok(2048 + u64::from((sequence - 1) % 4) * 16)
}
pub fn end_query(base: u64, sequence: u32) -> Result<[u32; 5], &'static str> {
    let address = base.checked_add(end_offset(sequence)?).filter(|a| *a < 1 << 40)
        .ok_or("GPU end timestamp address")?;
    Ok([0x200406c0, (address >> 32) as u32, address as u32, sequence, 0x0000f002])
}

/// Aggregate clock-converted interval bounds without dropping requests.
/// clocks: CPU submit, CPU end-query observation, CPU-before/PTIMER/CPU-after.
pub fn accumulate(stats: &mut [i128; 18], row: [u64; 6], end: u64, clocks: [u64; 5]) {
    stats[0] += 1;
    if stats[0] == 1 { stats[16] = i128::from(row[0]); }
    stats[17] = i128::from(row[0]);
    if row[5] != 1 || end < row[4] || clocks[4] < clocks[2]
        || clocks[3] < end || clocks[1] < clocks[0] { return; }
    stats[1] += 1;
    let [submit, observed, before, gpu_now, after] = clocks.map(i128::from);
    let begin = i128::from(row[2]); let end = i128::from(end);
    let values = [begin - gpu_now + before - submit, begin - gpu_now + after - submit,
        i128::from(row[3] - row[2]), i128::from(row[4] - row[3]), end - i128::from(row[4]),
        observed - after + gpu_now - end, observed - before + gpu_now - end];
    for (sum, value) in stats[2..9].iter_mut().zip(values) { *sum += value; }
    stats[9] = stats[9].max(after - before);
    stats[10] += observed - submit;
    for (max, value) in stats[11..16].iter_mut().zip([values[1],values[2],values[3],values[4],values[6]]) {
        *max = (*max).max(value);
    }
}

/// Keep both compiler and CPU ordering barriers on the clock/MMIO boundaries.
pub fn ordered_now(mut now: impl FnMut() -> u64, mut fence: impl FnMut()) -> u64 {
    fence(); let value = now(); fence(); value
}
pub fn clock_sample(mut now: impl FnMut() -> u64, mut read: impl FnMut(usize) -> u32,
    mut fence: impl FnMut()) -> Option<[u64; 3]> {
    let before = ordered_now(&mut now, &mut fence);
    let mut value = None;
    for _ in 0..8 {
        let high = read(0x9410); let low = read(0x9400);
        if high == read(0x9410) { value = Some(u64::from(high) << 32 | u64::from(low)); break; }
    }
    let after = ordered_now(&mut now, &mut fence);
    value.map(|gpu| [before, gpu, after])
}
