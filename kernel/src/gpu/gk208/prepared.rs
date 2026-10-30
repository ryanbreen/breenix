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

//! Prepared native terminal requests. The normal encoder runs at handoff;
//! the fatal path only patches glyph selectors and executes retained packets.
use alloc::vec::Vec;
use gpu_abi::{draw, pipeline, stream};

pub(crate) const COLS: usize = 80;
pub(crate) const ROWS: usize = 32;
pub(crate) const CELLS: usize = COLS * ROWS;

pub(crate) fn text_stream(target: u32, atlas: u32, glyph_width: u32) -> Result<Vec<u32>, &'static str> {
    let mut words = Vec::new();
    words.try_reserve_exact(7 + draw::HEADER + draw::INSTANCE * CELLS)
        .map_err(|_| "terminal stream allocation")?;
    stream::encode::begin(&mut words, u64::from(target));
    stream::encode::clear(&mut words, 0xff6e1216);
    let mut instances = Vec::new();
    instances.try_reserve_exact(CELLS).map_err(|_| "terminal instance allocation")?;
    for row in 0..ROWS {
        for col in 0..COLS {
            let x = 40 + col as u32 * glyph_width * 2;
            let y = 32 + row as u32 * 46;
            let mut params = [0; 16];
            params[..8].copy_from_slice(&[x, y, 0, 0, glyph_width, 16, 2, 1]);
            instances.push(draw::Instance {
                rect: [x as i32, y as i32, (x + glyph_width * 2) as i32, (y + 32) as i32],
                params,
            });
        }
    }
    draw::encode(&mut words, pipeline::BITMAP_TEXT, 0xffffebeb, u64::from(atlas), 0, 0, &instances);
    Ok(words)
}

/// The encoder's CB0 address selects each immutable quad's parameters. Only
/// param[2] (atlas x) is mutable. Validate packet and backing extents now.
pub(crate) fn glyph_offsets<'a>(chunks: impl IntoIterator<Item = &'a [u32]>, bytes: usize)
    -> Result<Vec<usize>, &'static str> {
    let mut offsets = Vec::new();
    offsets.try_reserve_exact(CELLS).map_err(|_| "terminal selector allocation")?;
    for words in chunks {
        let (mut at, mut selector, mut binding) = (0, None, None);
        while at < words.len() {
            let header = words[at];
            let count = ((header >> 16) & 0x1fff) as usize;
            if header & 0xe0000000 != 0x20000000 || count == 0 || count >= words.len() - at {
                return Err("terminal draw packet extent");
            }
            if header & 0x1fff == 0x2380 / 4 && count == 3 {
                let address = u64::from(words[at + 2]) << 32 | u64::from(words[at + 3]);
                let offset = usize::try_from(address).ok().and_then(|a| a.checked_add(8))
                    .filter(|a| a % 4 == 0 && a.checked_add(4).is_some_and(|end| end <= bytes))
                    .ok_or("terminal glyph outside backing")?;
                binding = Some(offset);
            }
            // Encoder binds CB0 to fragment slot 0 for parameters, then
            // selects the separate texture-handle CB at slot 15 (0xf1).
            if header & 0x1fff == 0x2490 / 4 && count == 1 && words[at + 1] == 1 {
                let offset = binding.ok_or("terminal constants binding absent")?;
                if selector.replace(offset).is_some() { return Err("terminal duplicate glyph selector"); }
            }
            at += 1 + count;
        }
        if offsets.len() == CELLS { return Err("terminal glyph count"); }
        offsets.push(selector.ok_or("terminal glyph selector absent")?);
    }
    if offsets.len() != CELLS { return Err("terminal glyph count"); }
    Ok(offsets)
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub shown: bool,
    pub observation: Option<u64>,
}

/// One small sequencing seam, shared by the real prepared packets and the host
/// failure check. There is deliberately no normal submission or release method.
pub(crate) trait Terminal {
    fn counter(&self) -> u64;
    fn frequency(&self) -> u64;
    fn latch(&mut self) -> bool;
    fn peers_idle(&self) -> bool;
    fn ready(&self) -> bool;
    fn patch(&mut self, deadline: u64) -> bool;
    fn drain(&mut self, deadline: u64) -> bool;
    fn draws(&self) -> usize;
    fn draw(&mut self, index: usize, deadline: u64) -> bool;
    fn flip(&mut self, deadline: u64) -> bool;
    fn observe(&mut self, deadline: u64) -> Option<u64>;
}

pub(crate) fn attempt(t: &mut impl Terminal) -> Outcome {
    let failed = Outcome { shown: false, observation: None };
    if !t.latch() { return failed; }
    let hz = t.frequency();
    if hz == 0 { return failed; }
    let Some(drain) = t.counter().checked_add(hz / 10) else { return failed; };
    loop {
        // At the boundary an undrained peer has exhausted its budget.
        if t.counter() >= drain { return failed; }
        if t.peers_idle() { break; }
        core::hint::spin_loop();
    }
    if !t.ready() { return failed; }
    let Some(deadline) = hz.checked_mul(2).and_then(|span| t.counter().checked_add(span))
        else { return failed; };
    if !t.patch(deadline) || !t.drain(deadline) { return failed; }
    for i in 0..t.draws() {
        if t.counter() >= deadline || !t.draw(i, deadline) { return failed; }
    }
    if t.counter() >= deadline || !t.flip(deadline) { return failed; }
    let observation = if t.counter() < deadline { t.observe(deadline) } else { None };
    Outcome { shown: true, observation }
}
