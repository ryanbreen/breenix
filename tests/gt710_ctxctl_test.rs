/*
 * Copyright 2011 Red Hat Inc.
 * Copyright 2012 Red Hat Inc.
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
// Register oracle: nouveau v6.8 gf100.c init_ctxctl_int/init_csdata,
// hub.fuc init/main_done, gpc.fuc init (MIT), envytools f102b823
// falcon.xml and ctxctl.xml (MIT via copyright.xml); license headers checked.
extern crate alloc;
#[path = "../libs/libbreenix/src/crypto/sha256.rs"]
pub mod sha256;
pub mod block { pub mod install { pub use crate::sha256; } }
#[path = "../kernel/src/gpu/gk208/gr/firmware.rs"]
pub mod firmware;
#[path = "../kernel/src/gpu/gk208/gr/tables.rs"]
pub mod tables;
#[path = "../kernel/src/gpu/gk208/gr/topology.rs"]
pub mod topology;
#[path = "../kernel/src/gpu/gk208/gr/ctxctl.rs"]
pub mod ctxctl;
use std::collections::BTreeMap;
use ctxctl::{Access, Controller, reset, BOUND_US};

#[derive(Clone, Debug, PartialEq)]
enum Event { Read(u32, u32), Write(u32, u32) }
struct Mock {
    regs: BTreeMap<u32,u32>, memory: BTreeMap<(u32,bool,u32),u32>,
    index: BTreeMap<(u32,bool),u32>, events: Vec<Event>,
    now: u64, frozen: bool, corrupt: Option<(u32,bool,u32)>,
    ready: bool, gpc_ready: bool, reply: bool, pgob_stuck: bool,
}
impl Mock {
    fn new() -> Self {
        Self { regs: BTreeMap::from([(0x200,0xa5010100), (0x409604,0x10001),
                (0x502608,2), (0x500c30,3), (0x409108,0x20040), (0x502108,0x20040),
                (0x120074,1), (0x100800,1)]), memory: BTreeMap::new(),
            index: BTreeMap::new(), events: vec![], now:0, frozen:false,
            corrupt:None, ready:true, gpc_ready:true, reply:true, pgob_stuck:false }
    }
    fn port(a:u32) -> Option<(u32,bool,bool)> {
        let base=a & !0xfff;
        if ![0x409000,0x502000,0x41a000].contains(&base) { return None; }
        match a & 0xfff {
            0x180=>Some((base,true,false)), 0x184=>Some((base,true,true)),
            0x1c0=>Some((base,false,false)), 0x1c4=>Some((base,false,true)), _=>None
        }
    }
}
impl Access for Mock {
    fn read(&mut self,a:u32)->u32 {
        let v=if let Some((b,c,true))=Self::port(a) {
            let index=*self.index.get(&(b,c)).unwrap_or(&0);
            let offset=index & 0xffff;
            let mut v=*self.memory.get(&(b,c,offset)).unwrap_or(&0);
            if self.corrupt==Some((b,c,offset)) { v^=1; }
            if index & 0x02000000!=0 { self.index.insert((b,c),index+4); }
            v
        } else { *self.regs.get(&a).unwrap_or(&0) };
        self.events.push(Event::Read(a,v)); v
    }
    fn write(&mut self,a:u32,v:u32) {
        self.events.push(Event::Write(a,v));
        if let Some((b,c,data))=Self::port(a) {
            let bases=if b==0x41a000 { vec![0x502000] } else { vec![b] };
            for b in bases {
                if data {
                    let index=*self.index.get(&(b,c)).unwrap_or(&0);
                    self.memory.insert((b,c,index & 0xffff),v);
                    if index & 0x01000000!=0 { self.index.insert((b,c),index+4); }
                } else { self.index.insert((b,c),v); }
            }
        }
        self.regs.insert(a,v);
        if matches!(a,0x20520|0x20524|0x20528|0x2052c|0x20530) && !self.pgob_stuck {
            self.regs.insert(a,v & 0x7fffffff);
        }
        if a==0x200 && v&0x1000==0 {
            for a in [0x409800,0x409804,0x502800,0x502804] { self.regs.insert(a,0); }
        }
        if a==0x409100 && v==2 {
            // FECS starts GPCCS itself for the unmodified source firmware.
            if self.gpc_ready { self.regs.insert(0x502800,0x80000000); }
            if self.ready { self.regs.insert(0x409800,0x80000000); }
            self.regs.insert(0x409804,0x4000);
        }
        if a==0x409504 && self.reply {
            let poison=self.regs[&0x409800];
            self.regs.insert(0x409800,poison|0x80000000);
        }
    }
    fn now_us(&mut self)->u64 { if !self.frozen { self.now+=100; } self.now }
    fn relax(&mut self) {}
}
fn topo()->topology::Topology<'static> {
    topology::decode(&[[0x409604,0x10001],[0x502608,2],[0x500c30,3]]).unwrap()
}
fn prepared(m:&mut Mock)->Controller {
    reset(m).unwrap(); Controller::upload(m,&topo(),[0x100000,0x101000]).unwrap()
}
fn image_words()->[(&'static [u32],u32,bool);4] {
    [(&firmware::FECS_CODE_BIN,0x409000,true),(&firmware::FECS_DATA_BIN,0x409000,false),
     (&firmware::GPCCS_CODE_BIN,0x502000,true),(&firmware::GPCCS_DATA_BIN,0x502000,false)]
}

#[test]
fn ctxctl_upload_and_readback_precede_start_and_match_embedded_images() {
    let mut m=Mock::new(); let mut c=prepared(&mut m);
    assert!(!m.events.contains(&Event::Write(0x409100,2)));
    for (n,(words,b,code)) in image_words().iter().enumerate() {
        let port=b+if *code {0x184} else {0x1c4};
        // Read observations must match the complete unpatched image before
        // context-register lists extend DMEM.
        let observed:Vec<_>=m.events.iter().filter_map(|e|match e {
            Event::Read(a,v) if *a==port=>Some(*v), _=>None
        }).take(words.len()).collect();
        assert_eq!(observed,*words);
        let upload_port=if *b==0x502000 {port-0x502000+0x41a000} else {port};
        let uploaded:Vec<_>=m.events.iter().filter_map(|e|match e {
            Event::Write(a,v) if *a==upload_port=>Some(*v), _=>None
        }).take(words.len()).collect();
        assert_eq!(uploaded,*words);
        let bytes:Vec<_>=words.iter().flat_map(|w|w.to_le_bytes()).collect();
        assert_eq!(c.readback_sha256[n],sha256::sha256(&bytes));
    }
    let before=m.events.len();
    let ready=c.start(&mut m).unwrap();
    assert!(ready.elapsed_us>0 && ready.elapsed_us<=BOUND_US);
    assert_eq!(ready.size,0x4000);
    assert_eq!(ready.status,vec![[0x409800,0x80000000],[0x502800,0x80000000]]);
    assert!(m.events[before..].contains(&Event::Write(0x409100,2)));
    assert!(!m.events.iter().any(|e|matches!(e,Event::Write(0x41a100|0x502100,2))));
    assert!(c.start(&mut m).is_err());
    assert_eq!(m.regs[&0x200]&!0x1000,0xa5010100);
    // The MMIO pack is applied before the first firmware word.
    assert!(m.events.iter().position(|e|*e==Event::Write(0x400080,0x003083c2)).unwrap()
        <m.events.iter().position(|e|matches!(e,Event::Write(0x409184,_))).unwrap());
}
#[test]
fn ctxctl_readback_mismatch_in_each_image_prevents_start() {
    for (words,b,code) in image_words() {
        let mut m=Mock::new(); m.corrupt=Some((b,code,(words.len() as u32-1)*4));
        reset(&mut m).unwrap();
        assert!(Controller::upload(&mut m,&topo(),[0x100000,0x101000]).is_err());
        assert!(!m.events.iter().any(|e|matches!(e,Event::Write(0x409100|0x41a100|0x502100,2))));
    }
}
#[test]
fn ctxctl_withheld_start_waits_to_bound_then_requires_fresh_upload() {
    let mut m=Mock::new(); let mut c=prepared(&mut m);
    let control=c.withheld_start(&mut m).unwrap();
    assert!(!control.started && !control.ready && !control.accepted);
    assert_eq!(control.reply,control.poison);
    assert!(control.elapsed_us>=BOUND_US);
    assert!(!m.events.contains(&Event::Write(0x409100,2)));
    // Reset, reapply tables and verify fresh uploads before the clean start.
    let mut c=prepared(&mut m); c.start(&mut m).unwrap();
}
#[test]
fn ctxctl_ready_without_start_is_refused() {
    let mut m=Mock::new(); let mut c=prepared(&mut m);
    m.regs.insert(0x409800,0x80000000);m.regs.insert(0x502800,0x80000000);
    assert!(c.withheld_start(&mut m).is_err());
}
#[test]
fn ctxctl_poison_precedes_both_mailbox_requests_and_unchanged_is_refused() {
    let mut m=Mock::new(); let mut c=prepared(&mut m);c.start(&mut m).unwrap();
    for (method,poison) in [(1,0x15555555),(2,0x2aaaaaaa)] {
        let begin=m.events.len();
        let reply=c.request(&mut m,method,0x80010000,poison).unwrap();
        assert_eq!(reply.method,method);assert_eq!(reply.poison,poison);
        assert_eq!(reply.reply,poison|0x80000000);assert_eq!(reply.size,0x4000);
        assert!(reply.elapsed_us>0 && reply.elapsed_us<=BOUND_US);
        let writes:Vec<_>=m.events[begin..].iter().filter(|e|matches!(e,Event::Write(..))).collect();
        assert_eq!(writes[..3],[&Event::Write(0x409800,poison),
            &Event::Write(0x409500,0x80010000),&Event::Write(0x409504,method)]);
    }
    m.reply=false; let begin=m.now;
    assert!(c.request(&mut m,1,0x80010000,0x15555555).is_err());
    assert!(m.now-begin>=BOUND_US && m.now-begin<BOUND_US+1000);
}
#[test]
fn ctxctl_stale_reply_withheld_request_is_never_accepted() {
    let mut m=Mock::new(); let mut c=prepared(&mut m);c.start(&mut m).unwrap();
    let previous=c.request(&mut m,1,0x80010000,0x15555555).unwrap().reply;
    let begin=m.events.len(); let control=c.stale_reply(&mut m,previous).unwrap();
    assert_eq!(control.method,1);assert_eq!(control.planted,previous);
    assert_eq!(control.reply,previous);assert!(!control.accepted);
    assert!(control.elapsed_us>=BOUND_US);
    assert!(!m.events[begin..].iter().any(|e|matches!(e,Event::Write(0x409504,_))));
}
#[test]
fn ctxctl_missing_fecs_or_gpccs_ready_and_frozen_clock_return_errors() {
    for (fecs,gpc) in [(false,true),(true,false),(false,false)] {
        let mut m=Mock::new();m.ready=fecs;m.gpc_ready=gpc;
        let mut c=prepared(&mut m); let begin=m.now;
        let error=c.start(&mut m).unwrap_err();
        assert!(error.elapsed_us>=BOUND_US && m.now-begin<BOUND_US+1000);
    }
    let mut m=Mock::new();let mut c=prepared(&mut m);m.frozen=true;m.ready=false;
    assert!(c.start(&mut m).is_err());
    let mut m=Mock::new();m.frozen=true;assert!(reset(&mut m).is_err());
}
#[test]
fn ctxctl_bad_size_caps_and_busy_engine_are_errors() {
    let mut m=Mock::new();m.regs.insert(0x409108,0);reset(&mut m).unwrap();
    assert!(Controller::upload(&mut m,&topo(),[0x100000,0x101000]).is_err());
    let mut m=Mock::new();m.regs.insert(0x40060c,1);reset(&mut m).unwrap();
    let begin=m.now;assert!(Controller::upload(&mut m,&topo(),[0x100000,0x101000]).is_err());
    assert!(m.now-begin<BOUND_US+1000);
    let mut m=Mock::new();let mut c=prepared(&mut m);c.start(&mut m).unwrap();
    m.regs.insert(0x409804,0);
    assert!(c.request(&mut m,1,0x80010000,0).is_err());
}

// nouveau v6.8 pmu/gk208.c selects gk110_pmu_pgob; gr/gf100.c calls
// pgob(false) before oneinit reads GPC/PPC topology. Both files are MIT.
#[test]
fn ctxctl_power_on_exits_pgob_before_topology_without_resetting_fifo_or_ce() {
    let mut m = Mock::new();
    ctxctl::reset(&mut m).unwrap();
    let sequence: Vec<_> = m.events.iter().filter_map(|event| match event {
        Event::Write(a @ (0x20520 | 0x20524 | 0x20528 | 0x2052c | 0x20530), v) => Some((*a, *v)),
        _ => None,
    }).collect();
    assert_eq!(sequence, vec![
        (0x20520,0xfffffffc),(0x20524,0xfffffffe),(0x20524,0xfffffffc),
        (0x20524,0xfffffff8),(0x20524,0xffffffe0),(0x20530,0xfffffffe),
        (0x2052c,0xfffffffa),(0x2052c,0xfffffff0),(0x2052c,0xffffffc0),
        (0x2052c,0xffffff00),(0x2052c,0xfffffc00),(0x2052c,0xfffcfc00),
        (0x2052c,0xfff0fc00),(0x2052c,0xff80fc00),(0x20528,0xfffffffe),
        (0x20528,0xfffffffc),
    ]);
    assert!(m.now >= 50_000);
    assert_eq!(m.regs[&0x200], 0xa5011100);
    assert_eq!(m.regs[&0x10a78c] & 3, 0);
    assert!(m.events.iter().filter_map(|e| match e { Event::Write(0x200,v) => Some(v), _=>None })
        .all(|v| v & !(0x08000000 | 0x1000) == 0xa5010100));
    // Each sequencer command must finish before the next is submitted.
    for (a,v) in sequence {
        let pos = m.events.iter().position(|e| *e == Event::Write(a,v)).unwrap();
        assert_eq!(m.events[pos+1], Event::Read(a,v & 0x7fffffff));
    }
}

#[test]
fn ctxctl_pgob_timeout_stops_the_sequence_and_restores_temporary_controls() {
    let mut m = Mock::new();
    m.pgob_stuck = true;
    assert!(ctxctl::reset(&mut m).is_err());
    assert!(!m.events.iter().any(|e| matches!(e, Event::Write(0x20524,_))));
    assert_eq!(m.regs[&0x200] & 0x08001000, 0);
    assert_eq!(m.regs[&0x10a78c] & 3, 0);
    assert!(m.now < 2_100_000);
}
