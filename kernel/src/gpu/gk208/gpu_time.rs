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

//! Sparse device-clock observations for diagnosis.
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
