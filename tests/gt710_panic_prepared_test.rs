//! Execute the native terminal sequencer without scheduler time or ordinary queues.
#[path = "../kernel/src/gpu/gk208/prepared.rs"]
mod prepared;
use prepared::{attempt, Outcome, Terminal};
use std::cell::Cell;

struct Device {
    ticks: Cell<u64>, hz: u64, latched: bool, valid: bool, peer_until: u64,
    fail: Option<&'static str>, cost: u64, calls: Vec<(&'static str, u64)>,
    pinned: usize,
}
impl Device {
    fn new() -> Self { Self {ticks: Cell::new(0), hz: 1000, latched: false, valid: true,
        peer_until: 0, fail: None, cost: 1, calls: vec![], pinned: 5} }
    fn step(&mut self, name: &'static str, deadline: u64) -> bool {
        self.calls.push((name, deadline));
        self.ticks.set(self.ticks.get() + self.cost);
        self.fail != Some(name) && self.ticks.get() < deadline
    }
}
impl Terminal for Device {
    fn counter(&self) -> u64 { let n=self.ticks.get(); self.ticks.set(n+1); n }
    fn frequency(&self) -> u64 { self.hz }
    fn latch(&mut self) -> bool { let first=!self.latched; self.latched=true; first }
    fn peers_idle(&self) -> bool { self.ticks.get() >= self.peer_until }
    fn ready(&self) -> bool { self.valid }
    fn patch(&mut self, d:u64) -> bool { self.step("patch",d) }
    fn drain(&mut self,d:u64) -> bool { self.step("drain",d) }
    fn draws(&self) -> usize { 3 }
    fn draw(&mut self,i:usize,d:u64) -> bool { self.step(["clear","snapshot","bitmap_text"][i],d) }
    fn flip(&mut self,d:u64) -> bool { self.step("flip",d) }
    fn observe(&mut self,d:u64) -> Option<u64> {
        for name in ["read55","readaa","compare","binding"] {
            if !self.step(name,d) { return None; }
        }
        Some(0)
    }
}
#[test]
fn prepared_draw_flip_and_both_observations_share_one_deadline() {
    let mut d=Device::new(); d.peer_until=75;
    assert_eq!(attempt(&mut d),Outcome {shown:true,observation:Some(0)});
    assert_eq!(d.calls.iter().map(|x|x.0).collect::<Vec<_>>(),
        ["patch","drain","clear","snapshot","bitmap_text","flip","read55","readaa","compare","binding"]);
    let deadline=d.calls[0].1;
    assert!((2075..=2080).contains(&deadline));
    assert!(d.calls.iter().all(|c|c.1==deadline));
    assert_eq!(d.pinned,5);
    let count=d.calls.len();
    assert_eq!(attempt(&mut d),Outcome {shown:false,observation:None});
    assert_eq!(d.calls.len(),count);
}
#[test]
fn no_preparation_invalidated_state_and_peer_timeout_never_submit() {
    for invalid in [true,false] {
        let mut d=Device::new(); d.valid=!invalid;
        if !invalid {d.peer_until=101;}
        assert_eq!(attempt(&mut d),Outcome {shown:false,observation:None});
        assert!(d.latched && d.calls.is_empty());
        assert!(d.ticks.get()<=102);
        assert_eq!(d.pinned,5);
    }
}
#[test]
fn every_request_failure_stops_without_fallback_retry_or_release() {
    let names=["patch","drain","clear","snapshot","bitmap_text","flip","read55","readaa","compare","binding"];
    for (i,name) in names.iter().enumerate() {
        let mut d=Device::new();d.fail=Some(name);
        assert_eq!(attempt(&mut d),Outcome {shown:i>=6,observation:None},"{name}");
        assert_eq!(d.calls.len(),i+1,"{name}");
        assert!(d.latched);assert_eq!(d.pinned,5);
    }
}
#[test]
fn time_spent_drawing_is_not_replenished_for_flip_or_capture() {
    for cost in [399,500,999,2000] {
        let mut d=Device::new();d.cost=cost;
        let result=attempt(&mut d);
        assert_eq!(result.observation,None);
        assert!(d.calls.iter().all(|c|c.1==d.calls[0].1));
        assert_eq!(d.pinned,5);
        assert!(d.calls.len()<10);
    }
}
#[test]
fn wrapping_counter_and_missing_frequency_refuse_without_submission() {
    let mut d=Device::new();d.ticks.set(u64::MAX-10);
    assert_eq!(attempt(&mut d),Outcome {shown:false,observation:None});
    assert!(d.calls.is_empty());
    let mut d=Device::new();d.hz=0;
    assert_eq!(attempt(&mut d),Outcome {shown:false,observation:None});
    assert!(d.calls.is_empty());
}

extern crate alloc;
#[allow(dead_code)] // The kernel uses the rest of this same committed catalogue.
#[path = "../kernel/src/gpu/gk208/gr/shaders.rs"]
mod shaders;
#[allow(dead_code)] // Other native capability rows have independent callers.
#[path = "../kernel/src/gpu/gk208/caps.rs"]
mod caps;
#[allow(dead_code)] // Full encoder APIs used by production, beyond this stream check.
#[path = "../kernel/src/gpu/gk208/encode.rs"]
mod encode;
use gpu_abi::{format, stream::{Resolver, SourceInfo, TargetInfo}};
struct Bindings;
impl Resolver for Bindings {
    fn target(&self,h:u64)->Option<TargetInfo> {(h==42).then_some(TargetInfo {width:2560,height:1600,format:format::B8G8R8A8,invalid:false})}
    fn source(&self,h:u64)->Option<SourceInfo> {(h==43).then_some(SourceInfo {width:8*95,height:16,format:format::R8,invalid:false})}
    fn buffer_size(&self,_:u64)->Option<u64> {None}
    fn buffer_rect(&self,_:u64,_:u64)->[i32;4] {[0;4]}
}
impl encode::Bindings for Bindings {
    fn render_target(&self,h:u64)->Option<encode::Target> {
        self.target(h).map(|t| encode::Target {id:42,address:0x1_0000_0000,width:t.width,height:t.height,format:t.format,tile_mode:0x30})
    }
    fn sampled_source(&self,h:u64)->Option<encode::Target> {
        self.source(h).map(|t| encode::Target {id:43,address:0x1_0200_0000,width:t.width,height:t.height,format:t.format,tile_mode:0x10})
    }
    fn blend_scratch(&self,_:u32)->Option<encode::Target> {
        Some(encode::Target {id:44,address:0x1_0400_0000,width:2560,height:1600,format:format::B8G8R8A8,tile_mode:0x30})
    }
}
#[test]
fn prepared_stream_uses_the_shared_bitmap_catalogue_and_patchable_glyphs() {
    let words=prepared::text_stream(42,43,8);
    let frame=encode::translate(words.as_slice(),&Bindings).unwrap();
    let offsets=prepared::glyph_offsets(frame.chunks.iter().filter(|c|c.source.is_some() && c.copy.is_none())
        .map(|c|c.words.as_slice()),frame.data.len()).unwrap();
    assert_eq!(offsets.len(),80*32);
    let mut data=frame.data.clone();
    for (i,&at) in offsets.iter().enumerate() {
        assert_eq!(at%4,0);
        assert_eq!(u32::from_le_bytes(data[at..at+4].try_into().unwrap()),0);
        data[at..at+4].copy_from_slice(&(((i%95)*8) as u32).to_le_bytes());
    }
    assert_eq!(offsets.iter().copied().collect::<std::collections::BTreeSet<_>>().len(),2560);
    assert!(prepared::glyph_offsets([&[0u32][..]],frame.data.len()).is_err());
    assert!(prepared::glyph_offsets(frame.chunks.iter().filter(|c|c.source.is_some() && c.copy.is_none())
        .map(|c|c.words.as_slice()),0).is_err());
}
