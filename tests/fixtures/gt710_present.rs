/*
 * SPDX-FileCopyrightText: Copyright (c) 1993-2022 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
 * SPDX-License-Identifier: MIT
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
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL
 * THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
 * FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
 * DEALINGS IN THE SOFTWARE.
 */
extern crate alloc;
use alloc::{format, string::String, vec::Vec};
use std::cell::RefCell;
thread_local! {static EVENTS: RefCell<Vec<String>>=const {RefCell::new(Vec::new())}; static FAIL:RefCell<&'static str>=const {RefCell::new("")};}
fn event(s:String){EVENTS.with(|e|e.borrow_mut().push(s));}
fn events()->Vec<String>{EVENTS.with(|e|e.borrow().clone())}
fn fails(s:&str)->bool{FAIL.with(|f|*f.borrow()==s)}
macro_rules! serial_println {($($arg:tt)*)=>{println!($($arg)*)};}
pub(crate) use serial_println;
fn hash(_: &[u8])->String{"00".repeat(32)}
mod firmware_writes {pub fn count()->u64{17}}
#[derive(Default)] struct Registers {active:u64,shadow:u64,put:u32,raster:u32,failed:bool,pixels:Vec<u8>}
impl Registers {
 fn read(&self,r:usize)->u32{match r {0x640460=>(self.active>>8) as u32,0x640004=>self.put,0x616308=>1,0x616340=>self.raster,0x61c004=>1,_=>0}}
 fn delay_us(&mut self,_:u32){self.raster+=1<<16;}
}
mod modeset {pub struct PllLimits;pub struct Pll;pub struct Op;pub fn bar_flush(_:&mut super::Registers)->Result<(),&'static str>{Ok(())} pub fn wait(r:&mut super::Registers,_:usize,_:u32,v:u32)->Result<(),&'static str>{assert_eq!(r.put,v);Ok(())}}
mod memory {
 #[derive(Clone,Copy)] pub struct Range{pub start:u64,pub size:u64}
 pub struct Pramin<'a>{r:&'a mut super::Registers}
 impl<'a> Pramin<'a>{pub fn new(r:&'a mut super::Registers,_:u64)->Self{Self{r}} pub fn write(&mut self,a:u64,v:u32)->Result<(),&'static str>{let i=(a-0x800000) as usize;self.r.pixels[i..i+4].copy_from_slice(&v.to_le_bytes());Ok(())}}
}
fn write_words(_: &mut Registers,_:u64,_:u64,w:&[u32])->Result<(),&'static str>{assert_eq!(w,&[0,0,0,0]);Ok(())}
fn read_word(_: &mut Registers,_:u64,_:u64)->Result<u32,&'static str>{Ok(0)}
fn submit(r:&mut Registers,_:u64,push:u64,put:&mut u32,methods:&[(u32,u32)])->Result<(),&'static str>{
 assert_eq!(push,0xa00000);assert_eq!(r.put,*put);assert_eq!(methods.len(),3);
 assert_eq!(methods[0].0,0x460);assert_eq!(&methods[1..],&[(0x84,0x80000000),(0x80,0)]);
 r.shadow=u64::from(methods[0].1)<<8;*put+=24;r.put=*put;event(format!("submit-{:x}-{}",r.shadow,*put));Ok(())
}
fn complete(r:&mut Registers,_:u64,notifier:u64,_:&modeset::PllLimits,_:&modeset::Pll,route:(u32,u32,bool,bool),_:&[Vec<modeset::Op>;4])->Result<u32,&'static str>{
 assert_eq!(notifier,0xb00000);assert_eq!(route,(0,0,true,true));
 // Fail after latching too: restoration must use the advanced PUT either way.
 r.active=r.shadow;
 if fails("flip")&&!r.failed{r.failed=true;return Err("injected flip error");}Ok(1)
}
struct ConsoleSurface {vram:u64,width:usize,height:usize,pitch:usize,firmware_baseline:u64}
fn console_binding(c:&ConsoleSurface)->Result<(),&'static str>{event(format!("console-binding-{:x}",c.vram));Ok(())}
mod native {use super::*;
 // HANDLE
 pub fn fixture()->Evo {Evo{head:0,sor:0,total:1<<30,push:0xa00000,put:1200,notifier:0xb00000,surfaces:[memory::Range{start:0x400000,size:5120*1024},memory::Range{start:0x800000,size:5120*1024}],limits:modeset::PllLimits,coefficients:modeset::Pll,scripts:core::array::from_fn(|_|vec![])}}
 pub fn cursor(e:&Evo)->u32{e.put}
 pub fn console_binding(c:&ConsoleSurface)->Result<(),&'static str>{super::console_binding(c)}
}
mod copy {pub struct Rect {pub src:u64,pub dst:u64,pub width:u32,pub height:u32,pub src_pitch:u32,pub dst_pitch:u32}}
struct Hardware<'a>{regs:&'a mut Registers,total:u64,console:&'a ConsoleSurface}
mod channel {
 pub type DrawSession = Session;
 use super::*;
 pub struct Session;
 impl Session {
  pub fn map_scanout(&mut self,_:&mut Hardware<'_>)->Result<u64,&'static str>{event("map".into());Ok(0xc00000)}
  pub fn copy_rect(&mut self,h:&mut Hardware<'_>,r:copy::Rect)->Result<(),&'static str>{
   assert_eq!((r.src,r.dst,r.width,r.height,r.src_pitch,r.dst_pitch),(0xd00000,0xc00000+32*5120+32*4,256,64,256,5120));
   event("ce-copy".into());if fails("copy"){return Err("injected copy error");}
   for y in 0..64{for x in 0..64{let i=(y+32)*5120+128+x*4;h.regs.pixels[i..i+4].copy_from_slice(&(0xff000000u32+y as u32*64+x as u32).to_le_bytes());}}Ok(())
  }
  pub fn readback(&mut self,h:&mut Hardware<'_>,va:u64,n:usize)->Result<[Vec<u8>;2],&'static str>{assert_eq!((va,n),(0xc00000,5120*1024));assert_eq!(h.regs.active,0x800000);event("read-all".into());if fails("read"){return Err("injected read error");}let a=h.regs.pixels.clone();let mut b=a.clone();if fails("poison"){b[n-1]^=1;}Ok([a,b])}
 }
}
struct Receipt{epoch:u64,nonce:u32}
struct DrawCapture{receipt:Receipt}
#[derive(Default)] struct DrawEvidence{target:Option<u64>,draws:Vec<DrawCapture>,present_rows:String,present_readbacks:Vec<String>}
// FLOW
fn run(failure:&'static str)->(Result<(),&'static str>,Registers,native::Evo,DrawEvidence){
 EVENTS.with(|e|e.borrow_mut().clear());FAIL.with(|f|*f.borrow_mut()=failure);
 let mut regs=Registers{active:0x400000,put:1200,pixels:vec![0;5120*1024],..Registers::default()};
 let c=ConsoleSurface{vram:0x400000,width:1280,height:1024,pitch:5120,firmware_baseline:17};
 let mut h=Hardware{regs:&mut regs,total:1<<30,console:&c};
 let mut evo=native::fixture();
 let mut e=DrawEvidence{target:Some(0xd00000),draws:vec![DrawCapture{receipt:Receipt{epoch:42,nonce:77}}],..DrawEvidence::default()};
 let result=present_probe(&mut h,&mut channel::Session,&mut evo,&mut e);
 (result,regs,evo,e)
}
#[test]fn carry_cursor_and_private_surfaces(){let (r,regs,evo,e)=run("");r.unwrap();assert_eq!(native::cursor(&evo),1248);assert_eq!(regs.active,0x400000);assert_eq!(e.present_readbacks.len(),2);assert_eq!(e.present_rows.lines().count(),2048);assert!(e.present_rows.contains("GT710_DRAW_IMAGE present 170 1023 "));assert!(e.present_rows.len()<192*1024);
 println!("presentation rows={} total conservative bulk={}",e.present_rows.len(),865396+353652+512*1024+e.present_rows.len());
 assert!(865396+353652+512*1024+e.present_rows.len()<2*1024*1024);
 let mut decoded=[Vec::new(),Vec::new()];
 for line in e.present_rows.lines(){let f:Vec<_>=line.split_whitespace().collect();let n=if f[2]=="85"{0}else{1};for run in f[4].split(','){let(c,w)=run.split_once(':').unwrap();let count=usize::from_str_radix(c,16).unwrap();let word=u32::from_str_radix(w,16).unwrap();for _ in 0..count{decoded[n].extend_from_slice(&word.to_le_bytes());}}}
 assert_eq!(decoded[0],regs.pixels);assert_eq!(decoded[1],regs.pixels);let events=events();assert_eq!(events,["map","ce-copy","submit-800000-1224","read-all","submit-400000-1248","console-binding-400000"]);}
#[test]fn injected_flip_error_restores_console(){let(r,regs,evo,_)=run("flip");assert_eq!(r,Err("injected flip error"));assert_eq!(regs.active,0x400000);assert_eq!(native::cursor(&evo),1248);assert!(!events().contains(&"read-all".into()));assert_eq!(events().last().unwrap(),"console-binding-400000");}
#[test]fn copy_and_read_errors_restore_console(){for f in ["copy","read","poison"]{let(r,regs,_,_)=run(f);assert!(r.is_err());assert_eq!(regs.active,0x400000);assert_eq!(events().last().unwrap(),"console-binding-400000");}}

mod gpu_observation {pub fn read(p:&super::staging::Pages,offset:usize,out:&mut[u8]){for(i,v)in out.iter_mut().enumerate(){let at=offset+i;*v=p.frames[at/4096][at%4096];}}}
mod staging {
 use super::*;
 const READBACK_BYTES:usize=65536;
 const TRANSFER:u64=0x2001000;
 pub struct Pages{pub frames:Vec<Vec<u8>>}
 impl Pages{fn va(&self,i:usize)->u64{self.frames[i].as_ptr() as u64}fn publish(&self){}}
 pub struct Session{pub pages:Box<Pages>,pub copies:usize,pub omit:bool}
 impl Session {
 // READBACK
 fn transfer(&mut self,_:&mut Hardware<'_>,src:u64,dst:u64,count:usize)->Result<(),&'static str>{
  assert_eq!(dst,TRANSFER);assert!(count<=READBACK_BYTES);
  let poison=if self.copies<80{0x55}else{0xaa};
  for i in 0..count{assert_eq!(self.pages.frames[1+i/4096][i%4096],poison);}
  for i in 0..count-usize::from(self.omit){self.pages.frames[1+i/4096][i%4096]=((src-0xc00000+i as u64)%251) as u8;}
  self.copies+=1;Ok(())
 }
 }
}
#[test]fn complete_complementary_scanout_uses_bounded_dma_chunks(){
 for omit in [false,true]{let mut s=staging::Session{pages:Box::new(staging::Pages{frames:(0..17).map(|_|vec![0;4096]).collect()}),copies:0,omit};
 let mut regs=Registers::default();let c=ConsoleSurface{vram:0,width:1280,height:1024,pitch:5120,firmware_baseline:17};let mut h=Hardware{regs:&mut regs,total:1<<30,console:&c};
 let images=s.readback(&mut h,0xc00000,5120*1024).unwrap();assert_eq!(s.copies,160);assert!(s.copies+100<512);
 if omit{assert_ne!(images[0],images[1]);}else{assert_eq!(images[0],images[1]);for(i,b)in images[0].iter().enumerate(){assert_eq!(*b,(i%251)as u8);}}
 }
}
