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
//! K8 production startup. Hardware operations reuse the C3/C4/C5 hardware helpers.
//! MIT nouveau v6.8 clk/gk104.c, fb/ramgk104.c, pmu/{gt215,memx}.c;
//! notices and the per-file audited source revisions remain in the shared helpers.
use super::{Registers, c2, clock, memx, modeset, native, production_policy as policy, production_rom};
use policy::{Stage, Startup};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
static CONTROLLER: spin::Mutex<Option<Controller>> = spin::Mutex::new(None);
static TRIPPED: AtomicBool = AtomicBool::new(false);
static ARMED: AtomicBool = AtomicBool::new(false);

fn now_ns() -> u64 {
    let (s,n) = crate::time::get_monotonic_time_ns();
    s.saturating_mul(1_000_000_000).saturating_add(n)
}
struct Io(Registers);
impl c2::Access for Io {
    fn read(&mut self,a:u32)->u32 {self.0.read(a as usize)}
    fn write(&mut self,a:u32,v:u32) {self.0.write(a as usize,v)}
    fn wait(&mut self,a:u32,mask:u32,value:u32)->Result<(), &'static str> {
        let start=now_ns();
        loop {
            if self.0.read(a as usize)&mask==value {return Ok(());}
            if now_ns().saturating_sub(start)>=2_000_000_000 {return Err("clock switch/PLL lock timeout");}
            core::hint::spin_loop();
        }
    }
}
impl c2::observe::CounterAccess for Io {
    fn monotonic_ns(&mut self)->u64 {now_ns()}
    fn record(&mut self,_stage:&str,_raw:&[(u32,u32)]) {}
}
impl super::pmu::Access for Io {
    fn read(&mut self,a:u32)->u32 {self.0.read(a as usize)}
    fn write(&mut self,a:u32,v:u32) {self.0.write(a as usize,v)}
    fn now_us(&mut self)->u64 {now_ns()/1000}
    fn waited(&mut self,name:&'static str,raw:u32,us:u64,matched:bool) {
        if !matched {crate::serial_println!("GT710_CLOCK_WAIT name={} raw={:#x} elapsed_us={} matched=false",name,raw,us);}
    }
}
impl memx::Access for Io {
    fn read(&mut self,a:u32)->u32 {self.0.read(a as usize)}
    fn write(&mut self,a:u32,v:u32) {self.0.write(a as usize,v)}
    fn now_us(&mut self)->u64 {now_ns()/1000}
}
impl super::thermal::Access for Io {
    fn read(&mut self,a:u32)->u32 {self.0.read(a as usize)}
    fn write(&mut self,a:u32,v:u32) {self.0.write(a as usize,v)}
    fn record(&mut self,w:super::thermal::Write) {
        // Retain failed restore evidence without imposing a fuse-write echo check.
        if !w.retained && w.written!=w.readback {
            crate::serial_println!("GT710_SENSOR_ACCESS address={:#x} before={:#x} written={:#x} readback={:#x}",w.address,w.before,w.written,w.readback);
        }
    }
    fn fuse_note(&mut self,r:Result<u32,&'static str>) {
        if let Err(e)=r {crate::serial_println!("GT710_SENSOR_NOTE {}",e);}
    }
    fn sleep_ms(&mut self,ms:u32) {modeset::Io::delay_us(&mut self.0,ms*1000);}
}
struct Controller {
    base:usize, strap:u32, rom:Vec<u8>, post:Vec<(u32,u32)>, counter:Option<u32>,
    target:production_rom::Target, post_memory:u32,
}
impl Controller {
    fn temperature(&self)->Result<u32,&'static str> {
        super::thermal::temperature(&mut Io(Registers(self.base)))
    }
    fn cool(&self)->Result<(),&'static str> {
        if TRIPPED.load(Ordering::Acquire) || self.temperature()? >=85 {Err("temperature at least 85 C or guard latched")}
        else {Ok(())}
    }
    fn sample(&self)->clock::Sample {
        clock::sample(|a| if a==0x101000 {self.strap}else{Registers(self.base).read(a as usize)})
    }
    fn gpu_health(&self)->Result<(),&'static str> {
        let r=Registers(self.base);
        let disp=r.read(0x10a01c);let intr=r.read(0x10a008)&disp&!(disp>>16);
        if intr&!0xc0!=0 || r.read(0x259c)!=0 || r.read(0x400100)!=0 {return Err("GPU or PMU clock transition fault");}
        native::memx_display_guard(&r)
    }
    fn memory_pass(&mut self,phase:&str,former:u32,target:u32,intermediate:bool,recovery:bool)->Result<(),&'static str> {
        let frequency=if intermediate {former.min(target)}else{target};
        let mut io=Io(Registers(self.base));
        super::ce::with_clock_owner(|_,_,_| {
            if !recovery {self.cool()?;self.gpu_health()?;}
            let session=memx::Session::init(&mut io)?;
            let calculated=memx::calculate_pass(&self.rom,self.strap,former,target,intermediate,&mut |a|io.0.read(a as usize));
            let plan=match calculated {Ok(p)=>p,Err(e)=>{io.0.write(0x10a580,0);return Err(e);}};
            session.load(&mut io,&plan.words())?;
            for &(a,v) in &plan.cpu_before {io.0.write(a as usize,v);}
            // Recovery must still run when the display is already unhealthy.
            let before=native::memx_display_boundary(&mut io.0,phase,false);
            if !recovery {
                if let Err(e)=before {io.0.write(0x10a580,0);return Err(e);}
            }
            let reply=session.exec(&mut io);
            let health=native::memx_display_boundary(&mut io.0,phase,true);
            // Preserve C6's EXEC -> reply/health -> CPU-after ordering. The
            // CPU-after values are applied on a failed reply as well, before recovery.
            for &(a,v) in &plan.cpu_after {io.0.write(a as usize,v);}
            match reply {
                Ok(reply)=>crate::serial_println!("GT710_CLOCK_EXEC phase={} status=0 pause_ns={} pmu_input={:#x} coefficients={:?}",phase,reply[0],reply[1],plan.coefficients),
                Err(e)=>{crate::serial_println!("GT710_CLOCK_EXEC phase={} status=-1 reason={}",phase,e);return Err(e);}
            }
            if !policy::within_one_percent(self.sample().memory_khz,frequency) {return Err("memory readback more than 1 percent off");}
            health?;
            if !recovery {self.cool()?;self.gpu_health()?;}
            Ok(())
        })
    }
    fn record(&mut self,full:bool,reason:&str) {
        if !full {
            self.counter=super::ce::with_clock_owner(|_,_,_|c2::observe::measure(&mut Io(Registers(self.base))))
                .ok().map(|m|m.khz);
        }
        let s=self.sample();
        crate::serial_println!("GT710_CLOCK_STARTUP full={} gpc_khz={} gpc_counter_khz={} memory_khz={} temperature_c={} status={}",
            full,s.gpc_khz.unwrap_or(0),self.counter.unwrap_or(0),s.memory_khz.unwrap_or(0),self.temperature().unwrap_or(u32::MAX),reason);
    }
}
impl Startup for Controller {
    fn step(&mut self,stage:Stage)->Result<(),&'static str> {
        if stage!=Stage::Sensor {self.cool()?;}
        match stage {
            Stage::Sensor=>{
                super::thermal::setup(&mut Io(Registers(self.base)))?;
                self.cool()?;
                let base=self.base;
                crate::task::kthread::kthread_run(move ||guard_thread(base),"kgputherm")
                    .map_err(|_|"thermal guard worker creation failed")?;
                Ok(())
            }
            Stage::Pmu=>super::ce::with_clock_owner(|_,_,_| {
                let rings=super::pmu::start(&mut Io(Registers(self.base)),
                    include_bytes!("../../../assets/kepler/pmu.code.bin"),
                    include_bytes!("../../../assets/kepler/pmu.data.bin"))?;
                crate::serial_println!("GT710_CLOCK_PMU_READY send={:#x} recv={:#x}",rings.send,rings.recv);
                Ok(())
            }),
            Stage::Ram=>{
                let scripts=super::ram_init::scripts(&self.rom,self.strap).map_err(|_|"RAM init script decode failed")?;
                super::ce::with_clock_owner(|_,_,_| {
                    super::ram_init::execute(&mut Registers(self.base),&scripts);Ok(())
                })?;
                if !policy::within_one_percent(self.sample().memory_khz,self.post_memory) {return Err("RAM init memory readback changed");}
                self.gpu_health()
            }
            Stage::MemoryIntermediate=>self.memory_pass("intermediate",self.post_memory,self.target.memory_khz,true,false),
            Stage::MemoryTarget=>self.memory_pass("target",self.post_memory,self.target.memory_khz,false,false),
            Stage::Gpc=>{
                let regs=Registers(self.base);
                // MIT volt/gk104.c speedo read with the gf100 fuse access gates.
                regs.write(0x122634,0);
                let enable=regs.read(0x22400);let access=regs.read(0x21000);
                regs.write(0x22400,enable|0x800);regs.write(0x21000,access|1);
                let speedo=regs.read(0x214a8) as i32;
                regs.write(0x21000,access);regs.write(0x22400,enable);regs.write(0x122634,0x41);
                let (current,required)=production_rom::voltage(&self.rom,self.target.voltage_id,self.temperature()?,speedo,&mut |a|regs.read(a as usize))?;
                if current<required {return Err("current VID voltage below performance requirement");}
                crate::serial_println!("GT710_CLOCK_VOLTAGE current_uv={} required_uv={} voltage_id={}",current,required,self.target.voltage_id);
                let limits=c2::limits(&self.rom)?;
                let reference=clock::gpc_reference(|a|Registers(self.base).read(a as usize))?;
                let plan=c2::pll_plan("gpc",self.target.gpc_khz,&limits,reference)?;
                crate::serial_println!("GT710_CLOCK_GPC requested_khz={} calculated_khz={}",self.target.gpc_khz,plan.khz);
                super::ce::with_clock_owner(|_,_,_|c2::program(&mut Io(Registers(self.base)),&plan))
            }
            Stage::Verify=>{
                let s=self.sample();
                if !policy::within_one_percent(s.gpc_khz,self.target.gpc_khz) || !policy::within_one_percent(s.memory_khz,self.target.memory_khz) {
                    return Err("clock readback more than 1 percent off");
                }
                let m=super::ce::with_clock_owner(|_,_,_|c2::observe::measure(&mut Io(Registers(self.base))))?;
                self.counter=Some(m.khz);
                crate::serial_println!("GT710_CLOCK_COUNTER khz={} cycles={} interval_ns={} bracket_ns={} restored=true",m.khz,m.cycles,m.ptimer_ns,m.bracket_ns);
                if !policy::within_one_percent(Ok(m.khz),self.target.gpc_khz.div_ceil(16)) {return Err("GPC counter more than 1 percent off");}
                self.cool()?;
                native::clock_display_check(&Registers(self.base),"startup")
            }
        }
    }
    fn restore_gpc(&mut self)->Result<(),&'static str> {
        super::ce::with_clock_owner(|_,_,_|c2::observe::restore(&mut Io(Registers(self.base)),&self.post))
    }
    fn restore_memory(&mut self)->Result<(),&'static str> {
        // Issue both live-calculated nouveau downclock passes, including
        // after an uncertain target EXEC; use live RAM timings for restoration.
        let first=self.memory_pass("return-intermediate",self.target.memory_khz,self.post_memory,true,true);
        let last=self.memory_pass("return",self.target.memory_khz,self.post_memory,false,true);
        first.and(last)
    }
}
fn keep_post()->bool {
    // Boot-root option, read once; missing is the normal full-clock boot.
    let Ok(root)=crate::fs::ext2::root_fs_read() else {return false;};
    let Some(fs)=root.as_ref() else {return false;};
    let Ok(n)=fs.resolve_path("/etc/breenix/gt710-clocks") else {return false;};
    let Ok(inode)=fs.read_inode(n) else {return true;};
    if inode.is_dir() || inode.size()>16 {return true;}
    let Ok(bytes)=fs.read_file_content_coherent(n,&inode) else {return true;};
    bytes.as_slice()!=b"full\n" && bytes.as_slice()!=b"full"
}
pub(super) fn initialize() {
    let result=super::ce::with_clock_owner(|regs,_,_| {
        let base=regs.0;
        let rom=super::rom::read(&mut Registers(base))?;
        let strap=Registers(base).read(0x101000);
        let post=c2::observe::capture(&mut Io(Registers(base)));
        let target=production_rom::target(&rom,strap)?;
        let post_memory=clock::sample(|a|Registers(base).read(a as usize)).memory_khz?;
        Ok(Controller {base,strap,rom,post,counter:None,target,post_memory})
    });
    let mut controller=match result {Ok(c)=>c,Err(e)=>{
        crate::serial_println!("GT710_CLOCK_STARTUP full=false gpc_khz=0 gpc_counter_khz=0 memory_khz=0 temperature_c=0 status={}",e);return;
    }};
    let post=keep_post();
    let outcome=policy::start(&mut controller,post);
    if let Some(e)=outcome.failure {
        crate::serial_println!("GT710_CLOCK_FALLBACK reason={} gpc_restore={} memory_restore={}",e,
            outcome.gpc_restore.unwrap_or("ok"),outcome.memory_restore.unwrap_or("ok"));
    }
    controller.record(outcome.full,outcome.failure.unwrap_or(if post {"post-option"}else{"ok"}));
    *CONTROLLER.lock()=Some(controller);
    ARMED.store(outcome.full,Ordering::Release);
}
fn guard_thread(base:usize) {
    // As with the existing timer-sleeping kernel workers, enable preemption
    // before sampling and sleeping. This worker is outside interrupt paths.
    unsafe {crate::arch_enable_interrupts();}
    let mut guard=policy::Guard::new();
    let mut logged=0;
    loop {
        let t=super::thermal::temperature(&mut Io(Registers(base)));
        if guard.sample(t) {TRIPPED.store(true,Ordering::Release);}
        let now=now_ns();
        if now.saturating_sub(logged)>=1_000_000_000 || guard.tripped() {
            crate::serial_println!("GT710_CLOCK_TEMPERATURE monotonic_ns={} celsius={} tripped={}",now,t.unwrap_or(u32::MAX),guard.tripped());
            logged=now;
        }
        if guard.tripped() && ARMED.swap(false,Ordering::AcqRel) {
            if let Some(c)=CONTROLLER.lock().as_mut() {
                let gpc=c.restore_gpc();
                let memory=c.restore_memory();
                crate::serial_println!("GT710_CLOCK_GUARD trip_c=85 temperature_c={} gpc_restore={} memory_restore={}",
                    t.unwrap_or(u32::MAX),gpc.err().unwrap_or("ok"),memory.err().unwrap_or("ok"));
                c.record(false,"thermal-guard-latched");
            }
        }
        let Some(tid)=crate::task::scheduler::current_thread_id() else {return;};
        crate::task::scheduler::with_current(|s|s.block_current_for_timer(now_ns().saturating_add(250_000_000)));
        crate::task::scheduler::yield_current();
        loop {
            crate::arch_halt_with_interrupts();
            if crate::task::scheduler::thread_state(tid)!=Some(crate::task::thread::ThreadState::BlockedOnTimer) {break;}
        }
    }
}
