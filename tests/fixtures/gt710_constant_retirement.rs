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

extern crate alloc;
#[path = "ROOT/kernel/src/gpu/gk208/gr/shaders.rs"] pub mod shaders;
#[path = "ROOT/kernel/src/gpu/gk208/encode.rs"] pub mod encode;
#[path = "ROOT/kernel/src/gpu/gk208/caps.rs"] pub mod caps;

// Hardware packet expectations: MIT NVIDIA open-gpu-doc cla197.h at
// 9fdf5c4062007929d9f4e6cbad9c9771fe61b880, WAIT_FOR_IDLE 0x110,
// UNBIND_ALL 0x10f4 bit 8, CB selector 0x2380-0x2388, CB bind 0x2490.
// The production publisher and encoder run here; only VM attachment and DMA
// publication are substituted. This checks storage references, not GPU execution.
mod gk208 {
    pub use crate::encode;
    pub mod adapter { pub struct EncodedDraw { pub words: Vec<u32> } }
    pub mod queue { pub struct Queue; impl Queue { pub fn slot(n: u32) -> u32 { (n - 1) % 4 } } }
    pub mod ce {
        pub struct Resources;
        impl Resources {
            pub fn bind(&mut self, _: &mut (), directory: u64, instance: u64, epoch: u64) -> Result<(), &'static str> {
                assert_eq!((directory, instance, epoch), (0x100000, 0x851c000, 17)); Ok(())
            }
        }
        pub fn with_resources<T>(f: impl FnOnce(&mut Resources, &mut (), &mut ()) -> Result<T, &'static str>) -> Result<T, &'static str> {
            f(&mut Resources, &mut (), &mut ())
        }
    }
    pub mod gr {
        const SUBCHANNEL: u32 = 0;
        const RELEASE: u32 = 0x1000f010;
        // REPORT
        pub mod channel {
            const TARGET: u64 = 0x02000000;
            pub struct Range { pub start: u64 }
            pub struct Directory { pub range: Range }
            pub struct Space { pub directory: Directory }
            pub struct Context { pub space: Space }
            impl Context { pub fn space(&self) -> &Space { &self.space } }
            pub struct Identity { pub instance: u64, pub epoch: u64 }
            pub struct Report { pub identity: Identity }
            pub struct Session { pub context: Context, pub report: Report }
            pub struct NativeChannel { pub session: Session, pub quarantine: u64, pub sent: Vec<u32>, pub parking: Vec<u32> }
            impl NativeChannel {
                pub fn new() -> Self {
                    let t = crate::encode::Target { id: 1, address: 0x19200000, width: 1, height: 1,
                        format: gpu_abi::format::B8G8R8A8, tile_mode: 0 };
                    let (_, parking) = crate::encode::parking_frame(t,
                        crate::encode::Target { id: 2, address: 0x19201000, ..t }, 0x19100000, 0x18000000).unwrap();
                    Self { session: Session { context: Context { space: Space { directory: Directory { range: Range { start: 0x100000 } } } },
                        report: Report { identity: Identity { instance: 0x851c000, epoch: 17 } } },
                        quarantine: 0x18000000, sent: Vec::new(), parking }
                }
                // PUBLISH_DRAW
                fn publish_encoded(&mut self, sequence: u32, command: &mut Vec<u32>) -> Result<(), &'static str> {
                    command.extend([0x20010014, sequence]);
                    self.sent = command.clone(); Ok(())
                }
            }
        }
    }
}

fn retirement_state(words: &[u32], frame: core::ops::Range<u64>) {
    let (mut at, mut selector, mut bound, mut idle, mut unbound, mut reports) = (0, 0, [false; 16], false, false, 0);
    while at < words.len() {
        let count = ((words[at] >> 16) & 0x1fff) as usize;
        let method = (words[at] & 0x1fff) * 4;
        let data = &words[at+1..at+1+count];
        match method {
            0xd74 => { idle = false; unbound = false; }
            0x110 => idle = true,
            0x2490 => bound[(data[0] >> 4) as usize] = data[0] & 1 != 0,
            0x10f4 if data[0] & 0x100 != 0 => {
                assert!(idle, "unbind must follow completed shader use"); bound.fill(false); unbound = true;
            }
            0x2380 => selector = u64::from(data[1]) << 32 | u64::from(data[2]),
            0x1b00 => {
                reports += 1;
                // Frame storage may be unmapped as soon as these engine reports
                // and SET_REFERENCE complete. No CB binding may outlive it.
                assert!(unbound && !bound.iter().any(|b| *b), "draw release leaves frame constants bound");
                assert!(!frame.contains(&selector), "SCC selector still names retired frame storage");
                assert_eq!(selector, 0x02000100, "park outside the four receipt slots in the retained channel page");
            }
            _ => ()
        }
        at += 1 + count;
    }
    assert_eq!(reports, 3, "both epoch halves and the engine release must remain");
    assert_eq!(words[words.len()-2], 0x20010014, "retain the terminal FIFO reference");
}

#[test]
fn retired_solid_and_sampled_frames_leave_no_constant_binding_for_the_next_context() {
    use gpu_abi::{format, pipeline};
    let target = encode::Target { id: 19, address: 0x200000000, width: 64, height: 64,
        format: format::B8G8R8A8, tile_mode: 0x30 };
    let source = encode::Target { id: 20, address: 0x200100000, ..target };
    let mut channel = gk208::gr::channel::NativeChannel::new();
    for (n, pipe) in [pipeline::COPY, pipeline::SOLID, pipeline::NEAREST_SCALED].into_iter().enumerate() {
        let base = 0x105a20000 + n as u64 * 0x120000;
        let mut frame = encode::warm_frame(target, source, None, None, pipe).unwrap();
        frame.relocate(base).unwrap();
        for chunk in frame.chunks {
            let draw = gk208::adapter::EncodedDraw { words: chunk.words };
            channel.publish_draw(6480 + n as u32, &draw).unwrap();
            retirement_state(&channel.sent, base..base+frame.data.len() as u64);
        }
    }
    // A clipped frame still needs the real report, including after a sampled
    // frame has bound both fragment slots 0 and 15.
    channel.publish_draw(6483, &gk208::adapter::EncodedDraw { words: Vec::new() }).unwrap();
    retirement_state(&channel.sent, 0x105a20000..0x105b38000);
}
