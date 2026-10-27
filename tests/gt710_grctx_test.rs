/*
 * Copyright 2010 Red Hat Inc.
 * Copyright 2011 Red Hat Inc.
 * Copyright 2012 Red Hat Inc.
 * Copyright 2013 Red Hat Inc.
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
 * THE COPYRIGHT HOLDER(S) OR AUTHOR(S) BE LIABLE FOR ANY CLAIM, DAMAGES OR
 * OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
 * ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
 * OTHER DEALINGS IN THE SOFTWARE.
 *
 * Authors: Ben Skeggs
 */
/*
 * rnndb copyright 2006
 * Artur Huillet
 * Ben Skeggs
 * B. R.
 * Carlos Martin
 * Christoph Bumiller
 * Dawid Gajownik
 * Dmitry Baryshkov
 * Dmitry Eremin-Solenikov
 * EdB
 * Erik Waling
 * Francisco Jerez
 * Ilia Mirkin
 * jb17bsome
 * Jeremy Kolb
 * Laurent Carlier
 * Luca Barbieri
 * Maarten Maathuis
 * Marcelina Kościelnicka
 * Mark Carey
 * Matthieu Castet
 * nvidiaman
 * Patrice Mandin
 * Pekka Paalanen
 * Peter Popov
 * Richard Hughes
 * Rudi Cilibrasi
 * Serge Martin
 * Simon Raffeiner
 * Stephane Loeuillet
 * Stephane Marchesin
 * sturmflut
 * Sylvain Munaut
 * Victor Stinner
 * Wladmir van der Laan
 * Younes Manton
 *
 * Permission is hereby granted, free of charge, to any person obtaining
 * a copy of this software and associated documentation files (the
 * "Software"), to deal in the Software without restriction, including
 * without limitation the rights to use, copy, modify, merge, publish,
 * distribute, sublicense, and/or sell copies of the Software, and to
 * permit persons to whom the Software is furnished to do so, subject to
 * the following conditions:
 *
 * The above copyright notice and this permission notice (including the
 * next paragraph) shall be included in all copies or substantial
 * portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
 * EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
 * MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
 * IN NO EVENT SHALL THE COPYRIGHT OWNER(S) AND/OR ITS SUPPLIERS BE
 * LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
 * OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION
 * WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
 */
// Oracle: pinned MIT ctxgf100.c generate_main/generate, ctxgk208.c dispatch,
// ctxgf117.c attrib/rop_mapping, ctxgk104.c bundle/pagepool, gf100.c:336-338;
// hub.fuc:134-213,285-294,585-592,686-690 and com.fuc:314-333 (G6).
extern crate alloc;
#[path = "../libs/libbreenix/src/crypto/sha256.rs"]
pub mod sha256;
pub mod block {
    pub mod install {
        pub use crate::sha256;
    }
}
#[path = "../kernel/src/gpu/gk208/gr/context.rs"]
pub mod context;
#[path = "../kernel/src/gpu/gk208/gr/ctxctl.rs"]
pub mod ctxctl;
#[path = "../kernel/src/gpu/gk208/gr/firmware.rs"]
pub mod firmware;
#[path = "../kernel/src/gpu/gk208/gr/tables.rs"]
pub mod tables;
#[path = "../kernel/src/gpu/gk208/gr/topology.rs"]
pub mod topology;
use context::{Buffer, Io, Spec};
use std::collections::BTreeMap;
#[derive(Debug, PartialEq)]
enum Event {
    Write(u32, u32),
    Allocate(Spec),
    Poison(u64, u8),
    Point(u64),
    Request(u32),
    Readback(u64),
    Patch(Vec<[u32; 2]>),
    Header(u64, [u32; 2]),
}
struct Mock {
    regs: BTreeMap<u32, u32>,
    events: Vec<Event>,
    next: u64,
    image: Vec<u8>,
    poison: u8,
    now: u64,
    saves: usize,
    corrupt: Option<usize>,
    short: bool,
    size_change: bool,
    bad_header: bool,
}
impl Mock {
    fn new() -> Self {
        Self {
            regs: BTreeMap::from([
                (0x409804, 1280),
                (0x409700, 1), (0x40974c, 32), (0x409880, 1),
                (0x409908, 2), (0x409910, 32),
                (0x502700, 3), (0x50274c, 32),
                (0x502880, 1),
                (0x502908, 4),
                (0x502910, 32),
            ]),
            events: vec![],
            next: 0x1000000,
            image: vec![],
            poison: 0,
            now: 0,
            saves: 0,
            corrupt: None,
            short: false,
            size_change: false,
            bad_header: false,
        }
    }
}
impl ctxctl::Access for Mock {
    fn read(&mut self, a: u32) -> u32 {
        let v = *self.regs.get(&a).unwrap_or(&0);
        if a == 0x404170 {
            v & !0x10
        } else {
            v
        }
    }
    fn write(&mut self, a: u32, v: u32) {
        self.events.push(Event::Write(a, v));
        self.regs.insert(a, v);
    }
    fn now_us(&mut self) -> u64 {
        self.now += 100;
        self.now
    }
    fn relax(&mut self) {}
}
impl Io for Mock {
    fn allocate(&mut self, s: Spec) -> Result<Buffer, context::Error> {
        self.events.push(Event::Allocate(s));
        let b = Buffer {
            spec: s,
            va: self.next,
        };
        self.next += s.bytes.div_ceil(4096) * 4096 + 4096;
        self.next = (self.next + 4095) & !4095;
        Ok(if self.short && s.name == "golden-55" {
            Buffer {
                spec: Spec {
                    bytes: s.bytes - 4,
                    ..s
                },
                ..b
            }
        } else {
            b
        })
    }
    fn patch(&mut self, b: Buffer, p: &[[u32; 2]]) -> Result<(), context::Error> {
        assert_eq!(b.spec.name, "mmio");
        self.events.push(Event::Patch(p.to_vec()));
        Ok(())
    }
    fn point(&mut self, b: Buffer) -> Result<(), context::Error> {
        self.events.push(Event::Point(b.va));
        Ok(())
    }
    fn poison(&mut self, b: Buffer, p: u8) -> Result<(), context::Error> {
        self.events.push(Event::Poison(b.va, p));
        self.image = vec![p; b.spec.bytes as usize];
        self.poison = p;
        Ok(())
    }
    fn request(&mut self, m: u32, p: u32) -> Result<ctxctl::Reply, context::Error> {
        self.events.push(Event::Request(m));
        if m == 2 {
            self.saves += 1;
            for i in (256..384).chain(512..640).chain(768..896).chain(1024..1152) {
                self.image[i] = ((i * 19 + i / 9) & 255) as u8;
            }
            if self.saves == 2 {
                if let Some(i) = self.corrupt {
                    self.image[i] ^= 1;
                }
            }
        }
        Ok(ctxctl::Reply {
            method: m,
            poison: p,
            reply: p | 0x80000000,
            size: if self.size_change { 1284 } else { 1280 },
            elapsed_us: 100,
        })
    }
    fn readback(&mut self, b: Buffer) -> Result<[Vec<u8>; 2], context::Error> {
        self.events.push(Event::Readback(b.va));
        Ok([self.image.clone(), self.image.clone()])
    }
    fn capture_layout(&mut self) -> Result<context::SaveLayout, context::Error> {
        context::SaveLayout::capture(self)
    }
    fn runnable(
        &mut self,
        b: Buffer,
        image: &[u8],
        header: [u32; 2],
    ) -> Result<[u32; 2], context::Error> {
        assert_eq!(b.spec.name, "runnable");
        assert!(image[..256].iter().all(|v| *v == 0));
        self.events.push(Event::Header(b.va, header));
        Ok(if self.bad_header {
            [header[0] + 1, header[1]]
        } else {
            header
        })
    }
}
fn run(m: &mut Mock) -> Result<context::Golden, context::Error> {
    let t = topology::decode(&[[0x409604, 0x10001], [0x502608, 2], [0x500c30, 3]]).unwrap();
    context::generate(m, &t, 1280)
}
fn hex(d: &[u8]) -> String {
    d.iter().map(|v| format!("{v:02x}")).collect()
}
#[test]
fn source_sizes_alignment_and_patch_addresses_are_exact() {
    let mut m = Mock::new();
    let g = run(&mut m).unwrap();
    // ctxgk208.c dispatch; gf100.c:2035-2046 alignments and mmio 451-460.
    let specs: Vec<_> = g
        .buffers
        .iter()
        .map(|b| (b.spec.name, b.spec.bytes, b.spec.align))
        .collect();
    assert_eq!(
        specs,
        vec![
            ("bundle", 0x3000, 0x100),
            ("pagepool", 0x8000, 0x100),
            ("attrib", 0x2c8c0, 0x1000),
            ("mmio", 0x1000, 0x100),
            ("golden-55", 1280, 0x100),
            ("golden-aa", 1280, 0x100),
            ("runnable", 1280, 0x100)
        ]
    );
    let b = |name| g.buffers.iter().find(|b| b.spec.name == name).unwrap().va as u32;
    assert_eq!(
        &g.patches[..12],
        &[
            [0x40800c, b("pagepool") >> 8],
            [0x408010, 0x80000000],
            [0x419004, b("pagepool") >> 8],
            [0x419008, 0],
            [0x4064cc, 0x80000000],
            [0x408004, b("bundle") >> 8],
            [0x408008, 0x80000030],
            [0x418808, b("bundle") >> 8],
            [0x41880c, 0x80000030],
            [0x4064c8, 0x00c20200],
            [0x418810, 0x80000000 | b("attrib") >> 12],
            [0x419848, 0x10000000 | b("attrib") >> 12]
        ]
    );
    assert_eq!(
        &g.patches[12..16],
        &[
            [0x405830, 0x02180648],
            [0x4064c4, 0x0192ffff],
            [0x5030c0, 0x14300000],
            [0x5030e4, 0x0c900648]
        ]
    );
    assert_eq!(g.header, [g.patches.len() as u32, b("mmio") >> 8]);
    assert_eq!(g.saved_end, 1152);
    assert_eq!(g.layout.strand_base, 4);
    assert_eq!(g.layout.strand_words, 32);
}
#[test]
fn replay_order_expansion_classes_and_canonical_pack_digests_match_tables() {
    let mut m = Mock::new();
    let g = run(&mut m).unwrap();
    assert_eq!(
        g.lists.iter().map(|r| r.index).collect::<Vec<_>>(),
        [3, 4, 8, 5, 6, 7, 1, 2]
    );
    for r in &g.lists {
        let l = &tables::INIT_LISTS[r.index];
        assert_eq!(r.count, l.count);
        assert_eq!(hex(&r.sha256), l.sha256);
        assert_eq!(r.packs.len(), l.packs.len());
        for (actual, p) in r.packs.iter().zip(l.packs) {
            let mut wire = Vec::new();
            wire.extend(p.class_id.to_le_bytes());
            wire.extend((p.init.len() as u32).to_le_bytes());
            for e in p.init {
                for w in [e.addr, e.count, e.pitch, e.data] {
                    wire.extend(w.to_le_bytes());
                }
            }
            assert_eq!(actual.count, p.init.len());
            assert_eq!(actual.sha256, sha256::sha256(&wire));
        }
    }
    let writes: Vec<_> = m
        .events
        .iter()
        .filter_map(|e| {
            if let Event::Write(a, v) = e {
                Some([*a, *v])
            } else {
                None
            }
        })
        .collect();
    let expected: Vec<_> = [3, 4, 8, 5, 6, 7]
        .into_iter()
        .flat_map(|i| tables::INIT_LISTS[i].packs.iter())
        .flat_map(|p| p.init.iter())
        .flat_map(|e| (0..e.count).map(move |i| [e.addr + i * e.pitch, e.data]))
        .collect();
    let start = writes
        .windows(expected.len())
        .position(|w| w == expected)
        .expect("MMIO lists out of source order");
    assert!(start > 0); // FE reset/power precedes replay.
    let mut icmd = Vec::new();
    let mut data = 0;
    for w in &writes {
        if w[0] == 0x400204 {
            data = w[1];
        }
        if w[0] == 0x400200 {
            icmd.push([w[1], data]);
        }
    }
    let expected: Vec<_> = tables::INIT_LISTS[1]
        .packs
        .iter()
        .flat_map(|p| p.init.iter())
        .flat_map(|e| (0..e.count).map(move |i| [e.addr + i * e.pitch, e.data]))
        .collect();
    assert_eq!(icmd, expected);
    let mut methods = Vec::new();
    let mut data = 0;
    for w in &writes {
        if w[0] == 0x40448c {
            data = w[1];
        }
        if w[0] == 0x404488 {
            methods.push([w[1], data]);
        }
    }
    let expected: Vec<_> = tables::INIT_LISTS[2]
        .packs
        .iter()
        .flat_map(|p| {
            p.init.iter().flat_map(move |e| {
                (0..e.count).map(move |i| {
                    [
                        0x80000000 | p.class_id | ((e.addr + i * e.pitch) << 14),
                        e.data,
                    ]
                })
            })
        })
        .collect();
    assert_eq!(methods, expected);
}
#[test]
fn both_destinations_are_poisoned_before_bind_and_save_and_read_back_twice() {
    let mut m = Mock::new();
    let g = run(&mut m).unwrap();
    for (n, p) in [85, 170].into_iter().enumerate() {
        let b = g.buffers[4 + n];
        let at = m
            .events
            .iter()
            .position(|e| *e == Event::Poison(b.va, p))
            .unwrap();
        assert_eq!(
            &m.events[at + 1..at + 4],
            &[Event::Point(b.va), Event::Request(1), Event::Request(2)]
        );
        assert!(m.events[at + 4..].contains(&Event::Readback(b.va)));
        assert!(g.images[n][..256].iter().all(|v| *v == p));
        assert!(g.images[n][1152..].iter().all(|v| *v == p));
    }
    for range in [256..384,512..640,768..896,1024..1152] {
        assert_eq!(&g.images[0][range.clone()], &g.images[1][range]);
    }
    for range in [384..512,640..768,896..1024,1152..1280] {
        for (n,p) in [85,170].into_iter().enumerate() {
            assert!(g.images[n][range.clone()].iter().all(|b| *b==p));
        }
    }
    assert_ne!(g.images[0], g.images[1]);
    assert_eq!(g.hashes[0], sha256::sha256(&[&g.images[0][256..384], &g.images[0][512..640], &g.images[0][768..896], &g.images[0][1024..1152]].concat()));
    assert_eq!(g.hashes[0], g.hashes[1]);
    assert_eq!(g.queries.len(), 4);
}
#[test]
fn inconsistent_size_header_or_poisoned_extent_is_refused_without_masking() {
    let mut m = Mock::new();
    m.short = true;
    assert_eq!(
        run(&mut m).unwrap_err().reason,
        "allocated image size differs from FECS"
    );
    let mut m = Mock::new();
    m.size_change = true;
    assert_eq!(run(&mut m).unwrap_err().reason, "FECS size changed");
    let mut m = Mock::new();
    m.bad_header = true;
    assert_eq!(run(&mut m).unwrap_err().reason, "runnable header differs");
    for offset in [0,255,256,383,384,511,512,639,640,767,768,895,896,1023,1024,1151,1152,1279] {
        let mut m = Mock::new();
        m.corrupt = Some(offset);
        let e = run(&mut m).unwrap_err();
        assert_eq!(e.offset, offset);
        assert_ne!(e.first, e.second);
        assert_eq!(
            e.reason,
            if [256..384,512..640,768..896,1024..1152].iter().any(|r|r.contains(&offset)) {
                "golden saves differ"
            } else {
                "save changed poison outside data"
            }
        );
    }
}

#[test]
fn terminal_writes_cover_whole_quads_without_consuming_empty_strands_or_allocation_gaps() {
    // Firmware allocation: com.fuc:314-333. Terminal transfer footprint is
    // the GK208B hardware observation retained in fixtures/gt710_save_map.
    for (words, bytes) in [(0, 0), (1, 16), (2, 16), (3, 16), (4, 16),
                           (63, 256), (64, 256), (65, 272)] {
        let advance = (words / 64 + 1) * 256;
        let gpc = 512 + advance;
        let layout = context::SaveLayout {
            gpc: 0, strands: 1, strand_base: (gpc + 256) / 256, strand_words: 4,
            units: vec![
                context::UnitLayout { controller: 0x409000, mmio_base: 1, mmio_words: 1,
                    strands: vec![context::Strand { base: 2, words }] },
                context::UnitLayout { controller: 0x502000, mmio_base: gpc / 256, mmio_words: 1,
                    strands: vec![context::Strand { base: (gpc + 256) / 256, words: 4 }] },
            ],
        };
        let mask = layout.written(gpc + 512).unwrap();
        assert!(mask[256..272].iter().all(|v| *v));
        assert!(mask[272..512].iter().all(|v| !*v));
        assert_eq!(mask[512..gpc as usize].iter().filter(|v| **v).count(), bytes);
        assert!(mask[512..512 + bytes].iter().all(|v| *v));
        assert!(mask[512 + bytes..gpc as usize].iter().all(|v| !*v));
    }
    let mut m = Mock::new();
    m.regs.insert(0x40974c, 31);
    let error = run(&mut m).unwrap_err();
    assert_eq!(error.reason, "save terminal padding is not zero");
    assert!((380..384).contains(&error.offset));
}
