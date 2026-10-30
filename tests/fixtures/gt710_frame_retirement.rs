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
        // BIND
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
                    Self { session: Session { context: Context { space: Space { directory: Directory { range: Range { start: 0x100000 } } } },
                        report: Report { identity: Identity { instance: 0x851c000, epoch: 17 } } },
                        quarantine: 0x18000000, sent: Vec::new(), parking: crate::retained_bindings() }
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

// Retained channel storage supplied at the publisher boundary. Use the real
// sampled encoder to seed its bindings, including the descriptor's image VA.
const PARK: u64 = 0x19000000;
const IMAGE: u64 = 0x19200000;
fn retained_bindings() -> Vec<u32> {
    let t = encode::Target { id: 1, address: IMAGE, width: 1, height: 1,
        format: gpu_abi::format::B8G8R8A8, tile_mode: 0 };
    let mut frame = encode::warm_frame(t, encode::Target { id: 2, address: IMAGE+4096, ..t }, None, None, gpu_abi::pipeline::COPY).unwrap();
    frame.relocate(PARK).unwrap();
    let words = encode::channel_words(&frame.chunks[0].words, 0x18000000).unwrap();
    let (mut at, mut parked) = (0, Vec::new());
    while at < words.len() {
        let count = ((words[at] >> 16) & 0x1fff) as usize;
        if !matches!((words[at] & 0x1fff)*4, 0x1618 | 0xd74 | 0x1614) {
            parked.extend_from_slice(&words[at..at+count+1]);
        }
        at += count+1;
    }
    parked
}

#[test]
fn frame_release_retires_every_bound_region_across_sampled_solid_and_clipped_frames() {
    use std::collections::BTreeMap;
    use gpu_abi::{format, pipeline};
    assert_eq!(gk208::gr::bind(), [0x20010000, 0xa197]);
    // Whole binding inventory from encode::quad and resource::Resources holds:
    // code and stage offsets; CB selector and slots 0/15 (constants and handles);
    // vertex start/limit; channel VAB; TIC/TSC pools; RT; sampled image via TIC.
    // The encoder uses DRAW_ARRAYS, so it binds no index buffer. Reserved frame
    // bytes are never bound after channel_words replaces the VAB address.
    let inventory = [("code", 0x1608, 0x160c), ("vertex", 0x1c04, 0x1c08),
        ("vertex-limit", 0x1f00, 0x1f04), ("VAB", 0x17bc, 0x17c0),
        ("TIC", 0x1574, 0x1578), ("TSC", 0x155c, 0x1560),
        ("render-target", 0x800, 0x804), ("CB-selector", 0x2384, 0x2388)];
    let t = encode::Target { id: 19, address: 0x200000000, width: 64, height: 64,
        format: format::B8G8R8A8, tile_mode: 0x30 };
    let source = encode::Target { id: 20, address: 0x200100000, ..t };
    let mut channel = gk208::gr::channel::NativeChannel::new();
    assert!(!channel.parking.is_empty());
    let parked_target = encode::Target { id: 1, address: IMAGE, width: 1, height: 1,
        format: format::B8G8R8A8, tile_mode: 0 };
    let parked_source = encode::Target { id: 2, address: IMAGE+4096, ..parked_target };
    let (bytes, words) = encode::parking_frame(parked_target, parked_source,
        PARK+(1<<20), channel.quarantine).unwrap();
    assert!(bytes.len() <= 16384);
    assert_eq!(words, channel.parking, "production parking must match the retained sampled state");
    // The TIC in the retained input buffer points only at the retained image.
    let tic = 8192+512;
    let image_va = u64::from(u32::from_le_bytes(bytes[tic+4..tic+8].try_into().unwrap()))
        | (u64::from(u32::from_le_bytes(bytes[tic+8..tic+12].try_into().unwrap()) & 0xff)<<32);
    assert_eq!(image_va, IMAGE+4096);
    let (mut regs, mut bound) = (BTreeMap::<u32,u32>::new(), [false;16]);
    for (n, pipe) in [Some(pipeline::COPY), Some(pipeline::SOLID),
        Some(pipeline::NEAREST_SCALED), None].into_iter().enumerate() {
        let base = 0x105a20000 + n as u64 * 0x120000;
        let words = if let Some(pipe) = pipe {
            let mut frame = encode::warm_frame(t, source, None, None, pipe).unwrap();
            frame.relocate(base).unwrap();
            frame.chunks.remove(0).words
        } else { Vec::new() };
        channel.publish_draw(6480+n as u32, &gk208::adapter::EncodedDraw { words }).unwrap();
        // RAMFC reconstruction loses the old subchannel class. Each stream
        // must bind A197 before any graphics method, including a clipped frame.
        let mut class = None;
        let (mut at, mut reports, mut draws, mut idle, mut unbound) = (0,0,0,false,false);
        while at < channel.sent.len() {
            let words = &channel.sent;
            let count = ((words[at] >> 16) & 0x1fff) as usize;
            let method = (words[at]&0x1fff)*4;
            let data = &words[at+1..at+count+1];
            if method == 0 { class = Some(data[0]); }
            if method >= 0x100 {
                assert_eq!(class, Some(0xa197), "graphics method {method:#x} before A197 bind after RAMFC reset");
            }
            for (i, &v) in data.iter().enumerate() { regs.insert(method+4*i as u32,v); }
            assert!(!matches!(method, 0x17c8..=0x17d4), "non-indexed frame unexpectedly binds an index buffer");
            match method {
                0xd74 => { draws+=1; idle=false; unbound=false; }
                0x110 => idle=true,
                0x2490 => bound[(data[0]>>4) as usize]=data[0]&1!=0,
                0x10f4 if data[0]&0x100!=0 => {
                    assert!(idle); bound.fill(false); unbound=true;
                }
                0x1b00 => {
                    reports+=1;
                    assert!(idle && unbound && !bound.iter().any(|b| *b));
                    for &(name,hi,lo) in &inventory {
                        let address = (u64::from(*regs.get(&hi).unwrap())<<32)
                            | u64::from(*regs.get(&lo).unwrap());
                        let retained = (PARK..PARK+0x120000).contains(&address)
                            || address==IMAGE || address==0x18000000 || address==0x02000100;
                        assert!(retained, "release leaves {name} binding at {address:#x} outside retained channel storage (frame {base:#x})");
                    }
                    // TIC now resides in retained storage and its image address
                    // is IMAGE, not the caller's texture. The parked command
                    // must replace stage offsets too, not just the code base.
                    assert_eq!(regs[&0x2044], 0x30);
                    assert_eq!(regs[&0x2144], 0x2b0);
                    assert_eq!(regs[&0x2388], 0x02000100);
                }
                _=>()
            }
            at+=count+1;
        }
        assert_eq!(reports,3);
        assert_eq!(draws,usize::from(pipe.is_some()), "parking must never render");
        assert_eq!(channel.sent[channel.sent.len()-2],0x20010014);
    }
}
