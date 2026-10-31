/*
 * Copyright 2010 Red Hat Inc.
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
 * claim-lint:ok: MIT license condition; sources in THIRD_PARTY_NOTICES.
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
//! Probe-only descriptor and serialization for the C2 GPC controller.
use super::*;
use crate::gk208::{Registers,clock};
use alloc::{format,sync::Arc,vec::Vec};
use observe::CounterAccess;

struct State {base:usize,post:u32,limits:Limits,failed:bool,snapshot:Vec<(u32,u32)>,phase:u8,c3:bool}
static STATE:spin::Mutex<Option<State>>=spin::Mutex::new(None);
pub(in crate::gk208) fn initialize(regs:&Registers,rom:&[u8])->Result<(),&'static str> {
    let l=limits(rom)?;
    let post=clock::sample(|a|regs.read(a as usize)).gpc_khz?;
    if post!=810_000 {return Err("C2 requires measured POST GPC 810000");}
    let snapshot=observe::capture(&mut Io(Registers(regs.0)));
    print_snapshot("post-inventory",&snapshot);
    let mut io=Io(Registers(regs.0));
    let counters=observe::counter_snapshot(&mut io);
    io.record("post-inventory",&counters);
    let pmc=io.read(0x200);
    io.record("pm-post-inventory",&[(0x200,pmc)]);
    *STATE.lock()=Some(State {base:regs.0,post,limits:l,failed:false,snapshot,phase:0,c3:false});Ok(())
}
struct Io(Registers);
impl Access for Io {
    fn read(&mut self,a:u32)->u32 {self.0.read(a as usize)}
    fn write(&mut self,a:u32,v:u32) {
        crate::serial_println!("GT710_CLOCK_WRITE {{\"address\":{},\"value\":{}}}",a,v);
        self.0.write(a as usize,v)
    }
    fn wait(&mut self,a:u32,mask:u32,value:u32)->Result<(),&'static str> {
        let start=crate::time::get_monotonic_time_ns();
        loop {
            let raw=self.read(a);
            let now=crate::time::get_monotonic_time_ns();
            let elapsed=now.0.saturating_mul(1_000_000_000)+now.1-(start.0*1_000_000_000+start.1);
            let matched=raw&mask==value;
            if a==0x137000 && mask==0x20000 && (matched || elapsed>=2_000_000_000) {
                crate::serial_println!("GT710_PLL_LOCK {{\"locked\":{},\"elapsed_us\":{},\"control\":{}}}",matched,elapsed/1000,raw);
            }
            if matched {return Ok(());}
            if elapsed>=2_000_000_000 {
                if a==0x137000 && mask==0x20000 {return Err("GPC PLL lock timeout");}
                return Err("GPC switch/lock timeout");
            }
            core::hint::spin_loop();
        }
    }
}
impl observe::CounterAccess for Io {
    fn record(&mut self,stage:&str,raw:&[(u32,u32)]) {
        let raw:Vec<_>=raw.iter().map(|(a,v)|format!("[{a},{v}]")).collect();
        let prefix=if stage.starts_with("pm-") {"GT710_PMC"}else{"GT710_COUNTER"};
        crate::serial_println!("{} {{\"stage\":\"{}\",\"raw\":[{}]}}",prefix,stage,raw.join(","));
    }
    fn monotonic_ns(&mut self)->u64 {
        let (seconds,nanos)=crate::time::get_monotonic_time_ns();
        seconds.saturating_mul(1_000_000_000).saturating_add(nanos)
    }
}
fn print_snapshot(phase:&str,snapshot:&[(u32,u32)]) {
    let raw:Vec<_>=snapshot.iter().map(|(a,v)|format!("[{a},{v}]")).collect();
    crate::serial_println!("GT710_CLOCK_SNAPSHOT {{\"phase\":\"{}\",\"raw\":[{}]}}",phase,raw.join(","));
}
fn snapshot_records(s:&State,io:&mut Io,phase:&str)->Result<(),&'static str> {
    let snapshot=observe::capture(io);
    print_snapshot(phase,&snapshot);
    let diffs=observe::differences(&s.snapshot,&snapshot)?;
    for (a,before,after) in &diffs {
        crate::serial_println!("GT710_CLOCK_DIFF {{\"phase\":\"{}\",\"address\":{},\"post\":{},\"value\":{}}}",phase,a,before,after);
    }
    crate::serial_println!("GT710_CLOCK_DIFF_DONE phase={} count={}",phase,diffs.len());
    Ok(())
}
fn measure_record(io:&mut Io,phase:&str,target:Option<u32>)->Result<(),&'static str> {
    // C3 requires a valid counter within 1% of the K2 observed /16 ratio.
    // Cleanup and its readbacks are emitted by measure on either result.
    let measured=crate::gk208::ce::with_resources(|_,_,_|Ok(observe::measure(io)))?;
    let m=match measured {
        Ok(m)=>m,
        Err(reason)=>{
            crate::serial_println!("GT710_CLOCK_MEASURED {{\"phase\":\"{}\",\"domain\":\"gpc\",\"status\":\"unmeasured\",\"khz\":null,\"reason\":\"{}\"}}",phase,reason);
            return if target.is_some() {Err(reason)}else{Ok(())};
        }
    };
    crate::serial_println!("GT710_CLOCK_MEASURED {{\"phase\":\"{}\",\"domain\":\"gpc\",\"status\":\"ok\",\"khz\":{},\"method\":\"gpc0-pre-counter-ptimer\",\"cycles\":{},\"interval_ns\":{},\"ptimer_ns\":{},\"bracket_ns\":{},\"counter_restored\":true}}",
        phase,m.khz,m.cycles,m.ptimer_ns,m.ptimer_ns,m.bracket_ns);
    if let Some(target)=target {
        let expected=target.div_ceil(16);
        if m.khz.abs_diff(expected)>expected/100 {return Err("GPC counter differs from target/16 by over 1 percent");}
    }
    Ok(())
}
pub fn read_status()->Result<Vec<u8>,i32> {
    let state=STATE.lock();let s=state.as_ref().ok_or(-19)?;
    let sample=clock::sample(|a|Registers(s.base).read(a as usize));
    let khz=sample.gpc_khz.map_err(|_|-5)?;
    let mut status=format!("GT710_CLOCK_READ {{\"domain\":\"gpc\",\"readback_khz\":{khz},\"post_khz\":{},\"status\":\"ok\"}}\n",s.post).into_bytes();
    status.extend(crate::gk208::thermal::read_status());
    Ok(status)
}
fn set(bytes:&[u8])->Result<(),&'static str> {
    let text=core::str::from_utf8(bytes).map_err(|_|"invalid request")?;
    let fields:Vec<_>=text.split_whitespace().collect();
    if fields.len()!=2 {return Err("request needs two fields");}
    let (phase,requested,c3,name)=match (fields[0],fields[1]) {
        ("observe","post")=>(0,810_000,false,"post"),
        ("gpc","540000")=>(1,540_000,false,"low"),
        ("restore","post")=>(2,810_000,false,"restored"),
        ("c3","post")=>(0,810_000,true,"post"),
        ("pll","810000")=>(1,810_000,true,"A"),
        ("pll","1080000")=>(2,1_080_000,true,"B"),
        ("pll","1307000")=>(3,1_307_000,true,"C"),
        ("c3-restore","post")=>(4,810_000,true,"D"),
        _=>return Err("unsupported clock phase"),
    };
    let mut state=STATE.lock();let s=state.as_mut().ok_or("C2 not initialized")?;
    if s.failed {return Err("C2 stopped after failure");}
    if phase!=s.phase {return Err("C2c phase out of order");}
    if phase==0 {s.c3=c3;}else if s.c3!=c3 {return Err("clock sequence changed");}
    let mut io=Io(Registers(s.base));
    let result=(|| {
        if c3 && io.read(0x20400)>=85 {return Err("C3 temperature unavailable or at least 85 C");}
        if phase==0 {return Ok(());}
        if (!c3 && phase==2) || (c3 && phase==4) {
            return crate::gk208::ce::with_resources(|_,_,_|observe::restore(&mut io,&s.snapshot));
        }
        if !c3 && (requested>s.post.min(810_000) || requested<540_000) {return Err("outside C2 POST envelope");}
        let refclk=clock::gpc_reference(|a|io.read(a))?;
        let vco=clock::gpc_vco(|a|io.read(a))?;
        let p=if c3 {pll_plan("gpc",requested,&s.limits,refclk)?}
            else {plan("gpc",requested,s.post,&s.limits,refclk,vco)?};
        if !c3 && (p.khz>s.post || p.khz>810_000) {return Err("calculated clock above POST");}
        let ctrl=io.read(0x137250);
        if ctrl&0x80000000!=0 && ctrl&0x3f3f!=0 {return Err("POST final divider not unity");}
        // The caller's final GPU WAIT precedes the write. Serialize with the
        // native CE/GR owner so no other publication crosses the source switch.
        crate::gk208::ce::with_resources(|_,_,_| {
            let result=program(&mut io,&p);
            if result==Err("GPC PLL lock timeout") {
                let restored=observe::restore(&mut io,&s.snapshot);
                crate::serial_println!("GT710_CLOCK_RESTORE_AFTER_TIMEOUT status={}",restored.as_ref().err().copied().unwrap_or("ok"));
                restored?;
            }
            result
        })?;
        Ok(())
    })();
    // Register-derived readback is retained as evidence, separate from the counter.
    let sample=clock::sample(|a|io.read(a));
    let readback=sample.gpc_khz;
    let step=clock::gpc_pll_step(|a|io.read(a)).unwrap_or(1);
    let result=result.and_then(|()| {
        let actual=readback?;
        if if c3 {actual.abs_diff(requested)>requested/100}else{actual!=requested} {
            return Err("readback-mismatch");
        }
        // MIT gk104.c:397-406 tests bit17 only with bit4 clear. Once sync is
        // enabled, retain source/enable/sync checks without re-testing bit17.
        if c3 && (1..=3).contains(&phase) && (io.read(0x137100)&1!=1 || io.read(0x137000)&0x15!=0x15) {
            return Err("GPC PLL control or source mismatch");
        }
        Ok(())
    });
    // Retain the actual register differences even when restoration refuses.
    let snapshots=snapshot_records(s,&mut io,name).and_then(|()| {
        if c3 && (phase==0 || phase==4) && !observe::differences(&s.snapshot,&observe::capture(&mut io))?.is_empty() {
            return Err("C3 POST snapshot differs");
        }
        Ok(())
    });
    let result=result.and(snapshots).and_then(|()|measure_record(&mut io,name,if c3 {Some(requested)}else{None}));
    let status=result.as_ref().err().copied().unwrap_or("ok");
    crate::serial_println!("GT710_CLOCK_SET {{\"domain\":\"gpc\",\"requested_khz\":{},\"readback_khz\":{},\"status\":\"{}\",\"step_khz\":{}}}",
        requested,readback.map_or_else(|_|alloc::string::String::from("null"),|v|format!("{v}")),status,step);
    let raw:Vec<_>=sample.raw.iter().map(|(a,v)|format!("[{a},{v}]")).collect();
    crate::serial_println!("GT710_CLOCK_SET_RAW [{}]",raw.join(","));
    if result.is_err() {s.failed=true;}else{s.phase+=1;}
    result
}
#[derive(Debug)]
pub struct Control;
impl crate::ipc::object::IpcFile for Control {
    fn as_any(&self)->&dyn core::any::Any {self}
    fn fill_stat(&self,st:&mut crate::syscall::fs::Stat) {st.st_mode=0o100600;st.st_ino=510;st.st_nlink=1;}
    fn statfs_magic(&self)->u64 {0x9fa0}
    fn write(&self,bytes:&[u8],_:bool)->Result<u64,u64> {
        crate::process::privilege::require(crate::process::privilege::Operation::Trace)?;
        if bytes.len()>32 {return Err(22);}
        set(bytes).map_err(|_|5u64)?;Ok(bytes.len() as u64)
    }
}
pub fn descriptor()->Result<Arc<dyn crate::ipc::object::IpcFile>,u64> {
    Ok(Arc::try_new(Control).map_err(|_|12u64)?)
}
