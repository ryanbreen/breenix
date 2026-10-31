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

struct State {base:usize,post:u32,limits:Limits,failed:bool,snapshot:Vec<(u32,u32)>,phase:u8,c3:bool,pmu_started:bool,ram_started:bool,ram_scripts:Vec<Vec<crate::gk208::modeset::Op>>,memory_post:u32,strap:u32,rom:Vec<u8>,memx_started:bool,memx_phase:u8,memory_target:u32}
static STATE:spin::Mutex<Option<State>>=spin::Mutex::new(None);
pub(in crate::gk208) fn initialize(regs:&Registers,rom:&[u8])->Result<(),&'static str> {
    let l=limits(rom)?;
    let post=clock::sample(|a|regs.read(a as usize)).gpc_khz?;
    if post!=810_000 {return Err("C2 requires measured POST GPC 810000");}
    let strap=regs.read(0x101000);
    let ram_scripts=crate::gk208::ram_init::scripts(rom,strap).map_err(|_|"RAM init script predecode failed")?;
    let memory_post=clock::sample(|a|if a==0x101000 {strap}else{regs.read(a as usize)}).memory_khz?;
    let snapshot=observe::capture(&mut Io(Registers(regs.0)));
    print_snapshot("post-inventory",&snapshot);
    let mut io=Io(Registers(regs.0));
    let counters=observe::counter_snapshot(&mut io);
    io.record("post-inventory",&counters);
    let pmc=io.read(0x200);
    io.record("pm-post-inventory",&[(0x200,pmc)]);
    *STATE.lock()=Some(State {base:regs.0,post,limits:l,failed:false,snapshot,phase:0,c3:false,pmu_started:false,ram_started:false,ram_scripts,memory_post,strap,rom:rom.to_vec(),memx_started:false,memx_phase:0,memory_target:memory_post});Ok(())
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
    let sample=clock::sample(|a|if a==0x101000 {s.strap}else{Registers(s.base).read(a as usize)});
    let khz=sample.gpc_khz.map_err(|_|-5)?;
    let mut status=format!("GT710_CLOCK_READ {{\"domain\":\"gpc\",\"readback_khz\":{khz},\"post_khz\":{},\"status\":\"ok\"}}\n",s.post).into_bytes();
    let memory=sample.memory_khz.map_err(|_|-5)?;
    status.extend(format!("GT710_MEMORY_READ {{\"readback_khz\":{memory},\"post_khz\":{},\"status\":\"ok\"}}\n",s.memory_post).bytes());
    status.extend(crate::gk208::thermal::read_status());
    if s.memx_started {
        memx_guard(&Registers(s.base),s.strap,s.memory_target).map_err(|_|-5)?;
    } else if s.ram_started {
        ram_guard(&Registers(s.base),s.strap,s.memory_post).map_err(|_|-5)?;
    }
    if s.pmu_started {
        let regs=Registers(s.base);
        let disp=regs.read(0x10a01c);let intr=regs.read(0x10a008)&disp&!(disp>>16);
        let uas=regs.read(0x10a16c);let address=regs.read(0x10a168);
        status.extend(format!("GT710_PMU_STATE intr={intr:#010x} disp={disp:#010x} uas={uas:#010x} address={address:#010x}\n").bytes());
        // MIT gt215.c:147-174: UAS and unexpected pending interrupts are faults;
        // host receive/debug notifications (0x40/0x80) are not fault predicates.
        if intr&!0xc0!=0 {
            crate::serial_println!("GT710_C4_ERROR PMU fault intr={:#x} uas={:#x} address={:#x}",intr,uas,address);
            return Err(-5);
        }
    }
    Ok(status)
}
impl crate::gk208::pmu::Access for Io {
    fn read(&mut self,a:u32)->u32 {self.0.read(a as usize)}
    fn write(&mut self,a:u32,v:u32) {self.0.write(a as usize,v)}
    fn now_us(&mut self)->u64 {self.monotonic_ns()/1000}
    fn waited(&mut self,name:&'static str,raw:u32,us:u64,matched:bool) {
        crate::serial_println!("GT710_PMU_WAIT name={} raw={:#010x} elapsed_us={} matched={}",name,raw,us,matched);
    }
}
fn memx_inputs(io:&mut Io,phase:&str) {
    let raw=crate::gk208::memx_inputs::capture(|a|Access::read(io,a));
    let pairs:Vec<_>=raw.iter().map(|(a,v)|format!("[{a},{v}]")).collect();
    crate::serial_println!("GT710_MEMX_INPUTS {{\"phase\":\"{}\",\"raw\":[{}]}}",phase,pairs.join(","));
}
fn pmu_start(s:&mut State,capture_inputs:bool)->Result<(),&'static str> {
    if s.pmu_started || s.failed {return Err("PMU diagnostic already attempted");}
    s.pmu_started=true;
    let mut io=Io(Registers(s.base));
    if capture_inputs {memx_inputs(&mut io,"startup");}
    if Access::read(&mut io,0x20400)>=85 {s.failed=true;return Err("C4 temperature unavailable or at least 85 C");}
    let before:Vec<_>=[0x200,0x10a008,0x10a010,0x10a01c,0x10a04c,0x10a100,0x10a104,0x10a10c,0x10a110,0x10a168,0x10a16c,0x10a4d0,0x10a4dc].into_iter().map(|a|(a,Access::read(&mut io,a))).collect();
    print_snapshot("pmu-before-reset",&before);
    let code=include_bytes!("../../../../assets/kepler/pmu.code.bin");
    let data=include_bytes!("../../../../assets/kepler/pmu.data.bin");
    crate::serial_println!("GT710_PMU_UPLOAD code_bytes={} code_sha256={} data_bytes={} data_sha256={}",code.len(),crate::gk208::hash(code),data.len(),crate::gk208::hash(data));
    let result=crate::gk208::ce::with_resources(|_,_,_| {
        let rings=crate::gk208::pmu::start(&mut io,code,data)?;
        crate::serial_println!("GT710_PMU_READY send={:#010x} send_base={} send_size={} recv={:#010x} recv_base={} recv_size={}",rings.send,rings.send&65535,rings.send>>16,rings.recv,rings.recv&65535,rings.recv>>16);
        Ok(())
    }).and_then(|()| {
        if capture_inputs {
            // Capture immediately at readiness; no counter setup, clock request
            // or PMU command is needed for this read-only C5b observation.
            memx_inputs(&mut io,"pmu-ready");
            Ok(())
        } else {measure_record(&mut io,"pmu-post",Some(810_000))}
    });
    if result.is_err() {s.failed=true;}
    result
}
// RAM init is serialized against GPU work by ce::with_resources. The boot
// observer ends the run at its first fault; write echoes are records only.
struct RamIo {regs:Registers,strap:u32,memory:u32,error:Option<&'static str>,writes:u32}
impl crate::gk208::modeset::Io for RamIo {
    fn read(&mut self,a:usize)->u32 {self.regs.read(a)}
    fn write(&mut self,a:usize,v:u32) {
        if self.error.is_some() {return;}
        let before=self.regs.read(a);
        self.regs.write(a,v);
        let readback=self.regs.read(a);
        self.writes+=1;
        crate::serial_println!("GT710_RAM_INIT_WRITE {{\"ordinal\":{},\"selector\":{},\"address\":{},\"before\":{},\"value\":{},\"readback\":{}}}",
            self.writes,(self.regs.read(0x10f65c)>>4)&15,a,before,v,readback);
        self.error=ram_guard(&self.regs,self.strap,self.memory).err();
        if let Some(reason)=self.error {crate::serial_println!("GT710_C5C_ERROR {}",reason);}
    }
    fn delay_us(&mut self,us:u32) {crate::gk208::modeset::Io::delay_us(&mut self.regs,us);}
}
fn ram_guard(regs:&Registers,strap:u32,memory:u32)->Result<(),&'static str> {
    // Cache PEXTDEV_BOOT0: MIT bios/init.c:445-458 warns against rereading
    // the strap at each RAM_RESTRICT opcode on this BIOS generation.
    let sample=clock::sample(|a|if a==0x101000 {strap}else{regs.read(a as usize)});
    let temperature=regs.read(0x20400);
    let disp=regs.read(0x10a01c);let intr=regs.read(0x10a008)&disp&!(disp>>16);
    let mmu=regs.read(0x259c);let gr=regs.read(0x400100);
    let reason=if sample.memory_khz!=Ok(memory) {Some("C5c memory clock readback changed")}
        else if temperature>=85 {Some("C5c temperature invalid or at least 85 C")}
        else if intr&!0xc0!=0 || mmu!=0 || gr!=0 {Some("C5c GPU or PMU fault")}
        else {None};
    if let Some(reason)=reason {
        crate::serial_println!("GT710_C5C_ERROR guard memory_khz={:?} post_khz={} temperature={} pmu_intr={:#x} mmu={:#x} gr={:#x}",sample.memory_khz,memory,temperature,intr,mmu,gr);
        return Err(reason);
    }
    Ok(())
}
fn ram_start(s:&mut State)->Result<(),&'static str> {
    if !s.pmu_started || s.ram_started || s.failed {return Err("RAM init requires ready PMU and one attempt");}
    s.ram_started=true;
    let regs=Registers(s.base);
    ram_guard(&regs,s.strap,s.memory_post)?;
    let mut io=RamIo {regs,strap:s.strap,memory:s.memory_post,error:None,writes:0};
    crate::serial_println!("GT710_RAM_INIT_BEGIN scripts={} selector={:#x} memory_khz={}",s.ram_scripts.len(),io.regs.read(0x10f65c),s.memory_post);
    let before=clock::sample(|a|if a==0x101000 {s.strap}else{io.regs.read(a as usize)});
    print_snapshot("ram-init-before",&before.raw);
    let result=crate::gk208::ce::with_resources(|_,_,_| {
        crate::gk208::ram_init::execute(&mut io,&s.ram_scripts);
        match io.error {Some(reason)=>Err(reason),None=>Ok(())}
    }).and_then(|()|measure_record(&mut Io(Registers(s.base)),"ram-init-post",Some(810_000)));
    let after=clock::sample(|a|if a==0x101000 {s.strap}else{io.regs.read(a as usize)});
    print_snapshot("ram-init-after",&after.raw);
    if result.is_err() {s.failed=true;} else {
        crate::serial_println!("GT710_RAM_INIT_COMPLETE writes={} selector={:#x}",io.writes,io.regs.read(0x10f65c));
    }
    result
}
impl crate::gk208::memx::Access for Io {
    fn read(&mut self,a:u32)->u32 {self.0.read(a as usize)}
    fn write(&mut self,a:u32,v:u32) {self.0.write(a as usize,v)}
    fn now_us(&mut self)->u64 {self.monotonic_ns()/1000}
}
fn memx_guard(regs:&Registers,strap:u32,target:u32)->Result<(),&'static str> {
    let sample=clock::sample(|a|if a==0x101000 {strap}else{regs.read(a as usize)});
    if sample.memory_khz?.abs_diff(target)>target/100 {return Err("C5e memory outside stage target plus/minus 1 percent");}
    if regs.read(0x20400)>=85 {return Err("C5e temperature at least 85 C");}
    let disp=regs.read(0x10a01c);let intr=regs.read(0x10a008)&disp&!(disp>>16);
    if intr&!0xc0!=0 || regs.read(0x259c)!=0 || regs.read(0x400100)!=0 {return Err("C5e GPU or PMU fault");}
    crate::gk208::native::memx_display_guard(regs)
}
fn memory_record(regs:&Registers,strap:u32,phase:&str)->Result<u32,&'static str> {
    let sample=clock::sample(|a|if a==0x101000 {strap}else{regs.read(a as usize)});
    print_snapshot(phase,&sample.raw);
    let memory=sample.memory_khz?;
    crate::serial_println!("GT710_C5E_MEMORY phase={} readback_khz={} source={:#x} mpll_control={:#x} mpll_coef={:#x} ref_control={:#x} ref_coef={:#x} ref_frac={:#x}",
        phase,memory,regs.read(0x1373f4),regs.read(0x132000),regs.read(0x132004),regs.read(0x132020),regs.read(0x132024),regs.read(0x132034));
    Ok(memory)
}
fn memx_compare(plan:&crate::gk208::memx::Plan,phase:&str,target:bool) {
    // The text supplies expected operations to compare with the live plan.
    let text=include_str!("../../../../../docs/gui/gt710-c5b-memx.txt");
    let mut words=Vec::new();let mut in_words=false;let mut section=false;
    for row in text.lines() {
        if row.starts_with("## ") {
            if in_words {break;}
            section=if target {row.starts_with("## target:")}else{row.starts_with("## xition:")};
        }
        if section && row.starts_with("PMU words (headers") {in_words=true;continue;}
        if in_words {if let Some((_,v))=row.split_once(": ") {if let Ok(v)=u32::from_str_radix(v,16) {words.push(v);}}}
    }
    let mut expected=Vec::new();let mut p=0;
    while p<words.len() {
        let h=words[p];p+=1;let end=p+(h>>16) as usize;
        let stride=match h&65535 {3=>2,4=>4,5=>1,_=>0};
        if stride==0 {expected.push(crate::gk208::memx::Op {code:h&65535,args:Vec::new()});}
        else {for args in words[p..end].chunks_exact(stride) {expected.push(crate::gk208::memx::Op {code:h&65535,args:args.to_vec()});}}
        p=end;
    }
    let mut differences=0;
    for i in 0..plan.ops.len().max(expected.len()) {
        let actual=plan.ops.get(i);let offline=expected.get(i);
        if let Some(op)=actual {crate::serial_println!("GT710_MEMX_OP phase={} ordinal={} code={} args={:?}",phase,i+1,op.code,op.args);}
        if actual!=offline {
            differences+=1;
            crate::serial_println!("GT710_MEMX_DIFF phase={} ordinal={} live={:?} offline={:?}",phase,i+1,actual,offline);
        }
    }
    crate::serial_println!("GT710_MEMX_DIFF_DONE phase={} live_ops={} offline_ops={} differences={} live_words={} offline_words={} coefficients={:?}",phase,plan.ops.len(),expected.len(),differences,plan.words().len(),words.len(),plan.coefficients);
}
fn memx_pass(s:&mut State,phase:&str,former:u32,target:u32,intermediate:bool)->Result<(),&'static str> {
    let frequency=if intermediate {former.min(target)}else{target};
    let mut io=Io(Registers(s.base));
    let result=crate::gk208::ce::with_resources(|_,_,_| {
        memx_guard(&io.0,s.strap,s.memory_target)?;
        let session=crate::gk208::memx::Session::init(&mut io)?;
        crate::serial_println!("GT710_MEMX_INIT phase={} base={:#x} size={}",phase,session.base,session.size);
        let current=memory_record(&io.0,s.strap,&format!("{}-before",phase))?;
        let mut reads=Vec::new();
        let mut read=|a| {let v=Access::read(&mut io,a);reads.push((a,v));v};
        let calculated=if phase=="intermediate" {
            crate::gk208::memx::calculate(&s.rom,s.strap,current,&mut read)
        }else{crate::gk208::memx::calculate_pass(&s.rom,s.strap,former,target,intermediate,&mut read)};
        for (a,v) in reads {crate::serial_println!("GT710_MEMX_READ phase={} address={:#x} value={:#x}",phase,a,v);}
        let plan=match calculated {
            Ok(plan)=>plan,Err(e)=>{io.0.write(0x10a580,0);return Err(e);}
        };
        memx_compare(&plan,phase,frequency==900000);
        let words=plan.words();
        session.load(&mut io,&words)?;
        for &(a,v) in &plan.cpu_before {
            io.0.write(a as usize,v);
            crate::serial_println!("GT710_MEMX_CPU stage={} phase=before address={:#x} value={:#x}",phase,a,v);
        }
        crate::serial_println!("GT710_MEMX_EXEC_BEGIN phase={} frequency_khz={} words={}",phase,frequency,words.len());
        crate::gk208::native::memx_display_boundary(&mut io.0,phase,false)?;
        // EXEC reply[0] is pause nanoseconds; reply[1] is PMU_IN, not status.
        let reply=session.exec(&mut io)?;
        crate::serial_println!("GT710_MEMX_EXEC_REPLY phase={} status=0 pause_ns={} pmu_input={:#010x}",phase,reply[0],reply[1]);
        crate::gk208::native::memx_display_boundary(&mut io.0,phase,true)?;
        for &(a,v) in &plan.cpu_after {
            io.0.write(a as usize,v);
            crate::serial_println!("GT710_MEMX_CPU stage={} phase=after address={:#x} value={:#x}",phase,a,v);
        }
        memory_record(&io.0,s.strap,&format!("{}-after",phase))?;
        memx_guard(&io.0,s.strap,frequency)
    }).and_then(|()|measure_record(&mut io,&format!("{}-post",phase),Some(810_000)));
    if result.is_ok() {s.memory_target=frequency;}
    result
}
fn memx_start(s:&mut State,command:&[u8])->Result<(),&'static str> {
    if !s.ram_started || s.failed {return Err("C5e requires RAM init and a healthy prior step");}
    let result=match (command,s.memx_phase) {
        (b"memx intermediate",0)=>{s.memx_started=true;memx_pass(s,"intermediate",405000,900000,true)},
        (b"memx target",1)=>memx_pass(s,"target",405000,900000,false),
        // ramgk104.c:1136-1162: downclock xition then final target, with
        // a new live calculation after each EXEC; no synthetic readbacks.
        (b"memx return",2)=>memx_pass(s,"return-intermediate",900000,405000,true)
            .and_then(|()|memx_pass(s,"return",900000,405000,false)),
        _=>Err("C5e command out of sequence"),
    };
    if let Err(reason)=result {
        s.failed=true;
        crate::serial_println!("GT710_C5E_ERROR status=-1 reason={}",reason);
    }else{s.memx_phase+=1;}
    result
}
fn set(bytes:&[u8])->Result<(),&'static str> {
    if bytes==b"memx intermediate" || bytes==b"memx target" || bytes==b"memx return" {
        let mut state=STATE.lock();
        return memx_start(state.as_mut().ok_or("C5e not initialized")?,bytes);
    }
    if bytes==b"ram init" {
        let mut state=STATE.lock();
        return ram_start(state.as_mut().ok_or("C5c not initialized")?);
    }
    if bytes==b"pmu start" || bytes==b"pmu memx-inputs" {
        let mut state=STATE.lock();
        return pmu_start(state.as_mut().ok_or("C4 not initialized")?,bytes==b"pmu memx-inputs");
    }
    let text=core::str::from_utf8(bytes).map_err(|_|"invalid request")?;
    let fields:Vec<_>=text.split_whitespace().collect();
    if fields.len()!=2 {return Err("request needs two fields");}
    let (phase,requested,c3,name)=match (fields[0],fields[1]) {
        ("observe","post")=>(0,810_000,false,"post"),
        ("gpc","540000")=>(1,540_000,false,"low"),
        ("restore","post")=>(2,810_000,false,"restored"),
        ("c3","post")=>(0,810_000,true,"post"),
        ("c3-low","540000")=>(1,540_000,true,"low"),
        ("pll","810000")=>(1,810_000,true,"A"),
        ("pll","1080000")=>(2,1_080_000,true,"B"),
        ("pll","1307000")=>(3,1_307_000,true,"C"),
        ("c3-restore","post")=>(4,810_000,true,"D"),
        _=>return Err("unsupported clock phase"),
    };
    let mut state=STATE.lock();let s=state.as_mut().ok_or("C2 not initialized")?;
    if s.failed {return Err("C2 stopped after failure");}
    // C3-2 inserts the existing divider path before A and omits B.
    let divider=c3 && name=="low";
    if phase!=s.phase && !(c3 && phase==3 && s.phase==2) {return Err("C2c phase out of order");}
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
        let p=if c3 && !divider {pll_plan("gpc",requested,&s.limits,refclk)?}
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
        if c3 && requested>=810_000 && (1..=3).contains(&phase) && (io.read(0x137100)&1!=1 || io.read(0x137000)&0x15!=0x15) {
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
    if result.is_err() {s.failed=true;}else if !divider {s.phase=phase+1;}
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
