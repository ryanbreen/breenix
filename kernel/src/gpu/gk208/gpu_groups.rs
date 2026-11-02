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

//! Sparse draw-local queries. MIT Mesa nvc0_query_hw.c pipeline statistics
//! uses 0x0980a002 (ROP pixels); MIT NVIDIA cla197.h names report 19 PS
//! invocations. Counter snapshots do not reset or enable another counter.
use alloc::vec::Vec;

pub const MAX_DRAWS: usize = 4;
const RECORDS: usize = 7;
const BASE: u64 = 2112;
pub type Row = [u64; 32];
#[derive(Clone, Copy)]
pub struct State(pub [u32; 20]);
#[derive(Clone, Copy)]
struct Segment { start: usize, shader: usize, launch: usize, end: usize, post: usize, state: State }

pub fn span(sequence: u32) -> Result<(u64, u64), &'static str> {
    if sequence == 0 { return Err("draw-group sequence absent"); }
    let bytes = (MAX_DRAWS * RECORDS * 16) as u64;
    Ok((BASE + u64::from((sequence - 1) % 4) * bytes, bytes))
}
pub fn offset(sequence: u32, draw: usize, stage: usize) -> Result<u64, &'static str> {
    if draw >= MAX_DRAWS || stage >= RECORDS { return Err("draw-group query extent"); }
    Ok(span(sequence)?.0 + ((draw * RECORDS + stage) * 16) as u64)
}
fn query(sequence: u32, draw: usize, stage: usize) -> Result<[u32; 5], &'static str> {
    let address = 0x02000000 + offset(sequence, draw, stage)?;
    Ok([0x200406c0, (address >> 32) as u32, address as u32, sequence,
        if stage < 5 { 0xf002 } else { 0x0980a002 }])
}
fn packet(words: &[u32], at: usize) -> Result<(usize, u32), &'static str> {
    let h = *words.get(at).ok_or("draw-group packet absent")?;
    let n = ((h >> 16) & 0x1fff) as usize;
    if h >> 29 != 1 || h & 0xe000 != 0 || n == 0 || at + n + 1 > words.len() {
        return Err("draw-group packet extent");
    }
    Ok((at + n + 1, (h & 0x1fff) * 4))
}
pub fn instrument(words: Vec<u32>, sequence: u32) -> Result<(Vec<u32>, Vec<State>), &'static str> {
    if !super::gpu_clock::sampled(sequence) { return Ok((words, Vec::new())); }
    let mut selected = [None; MAX_DRAWS];
    let mut state = State([0; 20]);
    let (mut at, mut start, mut shader, mut launch, mut draws) = (0, 0, None, None, 0usize);
    while at < words.len() {
        let (end, method) = packet(&words, at)?;
        let data = &words[at + 1..end];
        match (method, data.len()) {
            (0x800, 9) => state.0[..4].copy_from_slice(&data[..4]),
            (0xa00, 6) => state.0[4..10].copy_from_slice(data),
            (0xe00, 3) => state.0[10..13].copy_from_slice(data),
            (0xc00, 4) => state.0[13..17].copy_from_slice(data),
            (0x2140, 2) => state.0[17] = data[1],
            (0xd74, 2) => state.0[18] = data[1],
            _ => (),
        }
        if matches!(method, 0x1608 | 0x21c | 0xda4) && shader.is_none() { shader = Some(at); }
        if method == 0x1618 {
            if launch.replace(at).is_some() { return Err("nested draw-group launch"); }
        }
        if method == 0x1614 {
            let shader_at = shader.take().ok_or("draw-group shader boundary absent")?;
            let launch_at = launch.take().ok_or("draw-group launch boundary absent")?;
            if !(start <= shader_at && shader_at <= launch_at && launch_at < end) {
                return Err("draw-group boundary order");
            }
            let mut post = end;
            while post < words.len() {
                let (next, m) = packet(&words, post)?;
                if m != 0x110 { break; }
                post = next;
            }
            if draws < MAX_DRAWS {
                selected[draws] = Some(Segment { start, shader: shader_at, launch: launch_at,
                    end, post, state });
            }
            draws += 1;
            start = post;
        }
        at = end;
    }
    if launch.is_some() { return Err("unterminated draw group"); }
    let count = draws.min(MAX_DRAWS);
    let mut out = Vec::new();
    out.try_reserve_exact(words.len() + count * RECORDS * 5).map_err(|_| "draw-group command allocation")?;
    let mut states = Vec::new();
    states.try_reserve_exact(count).map_err(|_| "draw-group state allocation")?;
    let mut copied = 0;
    for (i, segment) in selected.into_iter().flatten().enumerate() {
        out.extend_from_slice(&words[copied..segment.start]);
        out.extend(query(sequence, i, 5)?);
        out.extend(query(sequence, i, 0)?);
        out.extend_from_slice(&words[segment.start..segment.shader]);
        out.extend(query(sequence, i, 1)?);
        out.extend_from_slice(&words[segment.shader..segment.launch]);
        out.extend(query(sequence, i, 2)?);
        out.extend_from_slice(&words[segment.launch..segment.end]);
        out.extend(query(sequence, i, 3)?);
        out.extend(query(sequence, i, 6)?);
        out.extend_from_slice(&words[segment.end..segment.post]);
        out.extend(query(sequence, i, 4)?);
        let mut metadata = segment.state;
        metadata.0[19] = draws as u32;
        states.push(metadata);
        copied = segment.post;
    }
    out.extend_from_slice(&words[copied..]);
    Ok((out, states))
}
pub fn decode(sequence: u32, draw: usize, state: &State, raw: [[u32; 4]; 7]) -> Row {
    let stamp = |r: [u32; 4]| u64::from(r[2]) | u64::from(r[3]) << 32;
    let times: [u64; 5] = core::array::from_fn(|i| stamp(raw[i]));
    let before = u64::from(raw[5][0]) | u64::from(raw[5][1]) << 32;
    let after = u64::from(raw[6][0]) | u64::from(raw[6][1]) << 32;
    let valid = raw[..5].iter().all(|r| r[0] == sequence) && times[0] != 0
        && times.windows(2).all(|p| p[0] <= p[1])
        && stamp(raw[5]) != 0 && stamp(raw[5]) <= times[0]
        && stamp(raw[6]) >= times[3] && stamp(raw[6]) <= times[4];
    let mut row = [0; 32];
    row[0] = u64::from(sequence); row[1] = draw as u64;
    for (out, value) in row[2..22].iter_mut().zip(state.0) { *out = u64::from(value); }
    row[22..27].copy_from_slice(&times);
    row[27..].copy_from_slice(&[before, after, stamp(raw[5]), stamp(raw[6]), u64::from(valid)]);
    row
}
