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
use alloc::{format,string::String,vec::Vec};
use core::sync::atomic::{AtomicBool, Ordering};
static CONTROLLER: super::lock::Mutex<Option<Controller>> = super::lock::Mutex::new(None);
static SENSOR_ACCESS: super::lock::Mutex<()> = super::lock::Mutex::new(());
fn read_temperature(base:usize)->Result<u32,&'static str> {
    let _sensor=SENSOR_ACCESS.lock();
    super::thermal::temperature(&mut Io(Registers(base)))
}
static TRIPPED: AtomicBool = AtomicBool::new(false);
static HALTED: AtomicBool = AtomicBool::new(false);
pub(super) fn halted()->bool {HALTED.load(Ordering::Acquire)}
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
            if now_ns().saturating_sub(start)>=2_000_000_000 {
                crate::serial_println!("GT710_CLOCK_CHECK check=clock-wait register={:#x} actual={:#x} mask={:#x} expected={:#x} elapsed_ns={}",a,self.0.read(a as usize),mask,value,now_ns().saturating_sub(start));
                return Err("clock switch/PLL lock timeout");
            }
            core::hint::spin_loop();
        }
    }
}
impl c2::observe::CounterAccess for Io {
    fn endpoint_scope<R>(&mut self, f:impl FnOnce(&mut Self)->R)->R {
        crate::per_cpu::preempt_disable();
        let result=crate::arch_without_interrupts(|| f(self));
        crate::per_cpu::preempt_enable();
        result
    }
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
        if w.restoring && w.written!=w.readback {
            crate::serial_println!("GT710_SENSOR_ACCESS restoring=true address={:#x} before={:#x} written={:#x} readback={:#x} retained={}",w.address,w.before,w.written,w.readback,w.retained);
        }
    }
    fn fuse_note(&mut self,r:Result<u32,&'static str>) {
        if let Err(e)=r {crate::serial_println!("GT710_SENSOR_NOTE {}",e);}
    }
    fn sleep_ms(&mut self,ms:u32) {modeset::Io::delay_us(&mut self.0,ms*1000);}
}
struct Controller {
    base:usize, strap:u32, rom:Vec<u8>, post:Vec<(u32,u32)>, counter:Option<u32>,
    target:production_rom::Target, post_memory:u32, memory_changed:bool, gpc_changed:bool,
    check:&'static str, values:String, failure_saved:bool,
}
impl Controller {
    fn temperature(&self)->Result<u32,&'static str> {
        read_temperature(self.base)
    }
    fn context(&mut self,check:&'static str,values:String) {if !self.failure_saved {self.check=check;self.values=values;}}
    fn cool(&mut self)->Result<(),&'static str> {
        let temperature=self.temperature();
        if TRIPPED.load(Ordering::Acquire) || temperature.is_err() || temperature.is_ok_and(|t|t>=85) {
            self.context("temperature",format!("temperature={:?},threshold=85,latched={},adc={:#x},control_a={:#x},control_b={:#x}",
                temperature,TRIPPED.load(Ordering::Acquire),Registers(self.base).read(0x20400),Registers(self.base).read(0x20008),Registers(self.base).read(0x2000c)));
            Err(temperature.err().unwrap_or("temperature at least 85 C or guard latched"))
        }else{Ok(())}
    }
    fn sample(&self)->clock::Sample {
        clock::sample(|a| if a==0x101000 {self.strap}else{Registers(self.base).read(a as usize)})
    }
    fn gpu_health(&mut self)->Result<(),&'static str> {
        let r=Registers(self.base);
        if let Err(f)=policy::clock_health(&mut |a|r.read(a as usize)) {
            self.context(f.check,format!("register={:#x},actual={:#x},expected=0,pmc={:#x},pmu_dispatch={:#x},pmu_intr={:#x}",
                f.register,f.value,r.read(0x200),r.read(0x10a01c),r.read(0x10a008)));
            return Err(f.check);
        }
        if let Err(e)=native::memx_display_guard(&r) {
            self.context("display-health",format!("reason={};raw=preceding-GT710_C5F_HEALTH",e));return Err(e);
        }
        Ok(())
    }
    fn memory_pass(&mut self,phase:&str,former:u32,target:u32,intermediate:bool,recovery:bool)->Result<(),&'static str> {
        let frequency=if intermediate {former.min(target)}else{target};
        let mut io=Io(Registers(self.base));
        super::ce::with_clock_wait(|_,evo,console| {
            if !recovery {self.cool()?;self.gpu_health()?;}
            self.context("memx-session",format!("phase={},former={},target={},intermediate={}",phase,former,target,intermediate));
            let session=memx::Session::init(&mut io)?;
            self.context("memx-calculate",format!("phase={},former={},target={},intermediate={},strap={:#x}",phase,former,target,intermediate,self.strap));
            let calculated=memx::calculate_pass(&self.rom,self.strap,former,target,intermediate,&mut |a|io.0.read(a as usize));
            let plan=match calculated {Ok(p)=>p,Err(e)=>{io.0.write(0x10a580,0);return Err(e);}};
            self.context("memx-load",format!("phase={},words={},capacity={},coefficients={:?}",phase,plan.words().len(),session.size,plan.coefficients));
            session.load(&mut io,&plan.words())?;
            self.context("display-before-exec",format!("phase={},raw=GT710_C5F_HEALTH",phase));
            // Recovery must still run when the display is already unhealthy.
            let before=native::memx_display_boundary(&mut io.0,phase,false);
            if !recovery {
                if let Err(e)=before {io.0.write(0x10a580,0);return Err(e);}
            }
            self.memory_changed=true;
            for &(a,v) in &plan.cpu_before {io.0.write(a as usize,v);}
            self.context("memx-exec",format!("phase={},frequency={},coefficients={:?}",phase,frequency,plan.coefficients));
            let reply=session.exec(&mut io);
            let health=native::memx_display_boundary(&mut io.0,phase,true);
            // Preserve C6's EXEC -> reply/health -> CPU-after ordering. The
            // CPU-after values are applied on a failed reply as well, before recovery.
            for &(a,v) in &plan.cpu_after {io.0.write(a as usize,v);}
            match reply {
                Ok(reply)=>crate::serial_println!("GT710_CLOCK_EXEC phase={} status=0 pause_ns={} pmu_input={:#x} coefficients={:?}",phase,reply[0],reply[1],plan.coefficients),
                Err(e)=>{crate::serial_println!("GT710_CLOCK_EXEC phase={} status=-1 reason={}",phase,e);return Err(e);}
            }
            let readback=self.sample().memory_khz;
            self.context("memory-readback",format!("phase={},actual={:?},expected={},tolerance={}",phase,readback,frequency,frequency/100));
            if !policy::within_one_percent(readback,frequency) {return Err("memory readback more than 1 percent off");}
            self.context("display-after-exec",format!("phase={},raw=GT710_C5F_HEALTH",phase));
            health?;
            if !recovery {
                // Complete the retained console's first present after each EXEC
                // before accepting the transition or publishing full clocks.
                self.context("first-present",format!("phase={},head={},raw=GT710_C5F_HEALTH",phase,console.head));
                native::present(evo,&mut io.0,super::present::Surface {
                    resource:u32::MAX,address:console.vram,width:console.width as u32,
                    height:console.height as u32,pitch:console.pitch as u32,
                    block_height:0,format:0,dma:1,
                })?;
                self.cool()?;self.gpu_health()?;
            }
            Ok(())
        })
    }
    fn record(&mut self,full:bool,reason:&str) {
        if !full {
            self.counter=super::ce::with_clock_wait(|_,_,_|c2::observe::measure(&mut Io(Registers(self.base))))
                .ok().map(|m|m.khz);
        }
        let s=self.sample();
        crate::serial_println!("GT710_CLOCK_STARTUP full={} gpc_khz={} gpc_counter_khz={} memory_khz={} temperature_c={} status={}",
            full,s.gpc_khz.unwrap_or(0),self.counter.unwrap_or(0),s.memory_khz.unwrap_or(0),self.temperature().unwrap_or(u32::MAX),reason);
    }
}
impl Startup for Controller {
    fn changed(&self)->(bool,bool) {(self.gpc_changed,self.memory_changed)}
    fn step(&mut self,stage:Stage)->Result<(),&'static str> {
        let result=self.run_stage(stage);
        if result.is_err() {self.failure_saved=true;}
        result
    }
    fn restore_gpc(&mut self)->Result<(),&'static str> {
        self.restore_gpc_clock()
    }
    fn restore_memory(&mut self)->Result<(),&'static str> {self.restore_memory_clock()}
}
impl Controller {
    fn run_stage(&mut self,stage:Stage)->Result<(),&'static str> {
        if stage!=Stage::Sensor {self.cool()?;}
        match stage {
            Stage::Sensor=>{
                self.context("sensor-setup",format!("adc={:#x},control_a={:#x},control_b={:#x}",Registers(self.base).read(0x20400),Registers(self.base).read(0x20008),Registers(self.base).read(0x2000c)));
                {let _sensor=SENSOR_ACCESS.lock();super::thermal::setup(&mut Io(Registers(self.base)))?;}
                // Keep the guard alive even when the initial temperature
                // already refuses full clocks (K10's unconditional 95 C stop).
                let base=self.base;
                self.context("thermal-worker-start",String::from("worker=kgputherm"));
                crate::task::kthread::kthread_run(move ||guard_thread(base),"kgputherm")
                    .map_err(|_|"thermal guard worker creation failed")?;
                self.cool()
            }
            Stage::Pmu=>{
                self.context("pmu-start",format!("pmc={:#x},pmu_control={:#x}",Registers(self.base).read(0x200),Registers(self.base).read(0x10a100)));
                super::ce::with_clock_wait(|_,_,_| {
                let rings=super::pmu::start(&mut Io(Registers(self.base)),
                    include_bytes!("../../../assets/kepler/pmu.code.bin"),
                    include_bytes!("../../../assets/kepler/pmu.data.bin"))?;
                crate::serial_println!("GT710_CLOCK_PMU_READY send={:#x} recv={:#x}",rings.send,rings.recv);
                Ok(())
            })},
            Stage::Ram=>{
                self.context("ram-init-decode",format!("strap={:#x},rom_bytes={}",self.strap,self.rom.len()));
                let scripts=super::ram_init::scripts(&self.rom,self.strap).inspect_err(|e|crate::serial_println!("GT710_CLOCK_CHECK check=ram-init-decode error={:?}",e)).map_err(|_|"RAM init script decode failed")?;
                self.context("ram-init-execute",format!("scripts={},memory_before={:?}",scripts.len(),self.sample().memory_khz));
                super::ce::with_clock_wait(|_,_,_| {
                    super::ram_init::execute(&mut Registers(self.base),&scripts);Ok(())
                })?;
                let memory=self.sample().memory_khz;
                self.context("ram-init-readback",format!("actual={:?},expected={},tolerance={}",memory,self.post_memory,self.post_memory/100));
                if !policy::within_one_percent(memory,self.post_memory) {return Err("RAM init memory readback changed");}
                self.gpu_health()
            }
            Stage::MemoryIntermediate=>self.memory_pass("intermediate",self.post_memory,self.target.memory_khz,true,false),
            Stage::MemoryTarget=>self.memory_pass("target",self.post_memory,self.target.memory_khz,false,false),
            Stage::Gpc=>{
                let regs=Registers(self.base);
                // MIT volt/gk104.c speedo read with the gf100 fuse access gates.
                let speedo={
                let _sensor=SENSOR_ACCESS.lock();
                regs.write(0x122634,0);
                let enable=regs.read(0x22400);let access=regs.read(0x21000);
                regs.write(0x22400,enable|0x800);regs.write(0x21000,access|1);
                let speedo=regs.read(0x214a8) as i32;
                regs.write(0x21000,access);regs.write(0x22400,enable);regs.write(0x122634,0x41);
                speedo};
                self.context("voltage-decode",format!("voltage_id={},speedo={}",self.target.voltage_id,speedo));
                let (current,required)=production_rom::voltage(&self.rom,self.target.voltage_id,self.temperature()?,speedo,&mut |a|regs.read(a as usize))?;
                self.context("voltage-check",format!("current_uv={},required_uv={},voltage_id={}",current,required,self.target.voltage_id));
                if current<required {return Err("current VID voltage below performance requirement");}
                crate::serial_println!("GT710_CLOCK_VOLTAGE current_uv={} required_uv={} voltage_id={}",current,required,self.target.voltage_id);
                self.context("gpc-pll-limits",format!("rom_bytes={}",self.rom.len()));
                let limits=c2::limits(&self.rom)?;
                self.context("gpc-reference",format!("limits={:?}",limits));
                let reference=clock::gpc_reference(|a|Registers(self.base).read(a as usize))?;
                self.context("gpc-pll-calculate",format!("target={},reference={},limits={:?}",self.target.gpc_khz,reference,limits));
                let plan=c2::pll_plan("gpc",self.target.gpc_khz,&limits,reference)?;
                crate::serial_println!("GT710_CLOCK_GPC requested_khz={} calculated_khz={}",self.target.gpc_khz,plan.khz);
                self.context("gpc-program",format!("target={},calculated={},coefficient={:#x}",self.target.gpc_khz,plan.khz,plan.coefficient));
                self.gpc_changed=true;
                super::ce::with_clock_wait(|_,_,_|c2::program(&mut Io(Registers(self.base)),&plan))
            }
            Stage::Verify=>{
                let s=self.sample();
                self.context("clock-readback",format!("gpc_actual={:?},gpc_target={},memory_actual={:?},memory_target={}",s.gpc_khz,self.target.gpc_khz,s.memory_khz,self.target.memory_khz));
                if !policy::within_one_percent(s.gpc_khz,self.target.gpc_khz) || !policy::within_one_percent(s.memory_khz,self.target.memory_khz) {
                    return Err("clock readback more than 1 percent off");
                }
                self.context("gpc-counter",format!("target_khz={}",self.target.gpc_khz.div_ceil(16)));
                let m=super::ce::with_clock_wait(|_,_,_|c2::observe::measure(&mut Io(Registers(self.base))))?;
                self.counter=Some(m.khz);
                crate::serial_println!("GT710_CLOCK_COUNTER khz={} cycles={} interval_ns={} bracket_ns={} restored=true",m.khz,m.cycles,m.ptimer_ns,m.bracket_ns);
                self.context("gpc-counter-readback",format!("actual={},expected={}",m.khz,self.target.gpc_khz.div_ceil(16)));
                if !policy::within_one_percent(Ok(m.khz),self.target.gpc_khz.div_ceil(16)) {return Err("GPC counter more than 1 percent off");}
                self.cool()?;
                self.context("final-display",String::from("raw=GT710_C5F_HEALTH"));
                native::clock_display_check(&Registers(self.base),"startup")
            }
        }
    }
    fn restore_gpc_clock(&mut self)->Result<(),&'static str> {
        if !self.gpc_changed {return Ok(());}
        super::ce::with_clock_wait(|_,_,_|c2::observe::restore(&mut Io(Registers(self.base)),&self.post))?;
        self.gpc_changed=false;Ok(())
    }
    fn restore_memory_clock(&mut self)->Result<(),&'static str> {
        // Issue both live-calculated nouveau downclock passes, including
        // after an uncertain target EXEC; use live RAM timings for restoration.
        if !self.memory_changed {return Ok(());}
        let first=self.memory_pass("return-intermediate",self.target.memory_khz,self.post_memory,true,true);
        let last=self.memory_pass("return",self.target.memory_khz,self.post_memory,false,true);
        first.and(last)?;
        self.memory_changed=false;Ok(())
    }
}
fn keep_post()->bool {
    crate::boot::intent::command_line().is_some_and(|line|
        line.split_ascii_whitespace().any(|word|word=="gt710.clocks=post"))
}
pub(super) fn initialize() {
    let mut prepare_check="session-owner";
    let mut prepare_values=String::new();
    let result=super::ce::with_clock_wait(|regs,_,_| {
        let base=regs.0;
        prepare_check="rom-read";
        prepare_values=format!("prom_control={:#x}",Registers(base).read(0x1800));
        let rom=super::rom::read(&mut Registers(base))?;
        let strap=Registers(base).read(0x101000);
        let post=c2::observe::capture(&mut Io(Registers(base)));
        prepare_check="rom-type-perf-target";
        prepare_values=format!("strap={:#x},rom_bytes={}",strap,rom.len());
        let target=production_rom::target(&rom,strap).map_err(|failure| {prepare_check=failure.check;failure.reason})?;
        crate::serial_println!("GT710_CLOCK_TARGETS ram_type=DDR3 strap={:#x} gpc_khz={} memory_khz={} voltage_id={}",strap,target.gpc_khz,target.memory_khz,target.voltage_id);
        prepare_check="post-clock-readback";
        let sample=clock::sample(|a|Registers(base).read(a as usize));
        prepare_values=format!("gpc={:?},memory={:?},raw={:?}",sample.gpc_khz,sample.memory_khz,sample.raw);
        let post_memory=sample.memory_khz?;
        Ok(Controller {base,strap,rom,post,counter:None,target,post_memory,memory_changed:false,gpc_changed:false,check:"prepare",values:String::new(),failure_saved:false})
    });
    let mut controller=match result {Ok(c)=>c,Err(e)=>{
        crate::serial_println!("GT710_CLOCK_FALLBACK stage=Prepare check={} values={} reason={}",prepare_check,prepare_values,e);
        crate::serial_println!("GT710_CLOCK_STARTUP full=false gpc_khz=0 gpc_counter_khz=0 memory_khz=0 temperature_c=0 status={}",e);return;
    }};
    let post=keep_post();
    let outcome=policy::start(&mut controller,post);
    if let Some(e)=outcome.failure {
        let stage=outcome.failed_stage.map(|s|format!("{:?}",s)).unwrap_or_else(||String::from("unknown"));
        crate::serial_println!("GT710_CLOCK_FALLBACK stage={} check={} values={} reason={} gpc_restore={} memory_restore={}",stage,controller.check,controller.values,e,
            outcome.gpc_restore.unwrap_or("ok"),outcome.memory_restore.unwrap_or("ok"));
    }
    controller.record(outcome.full,outcome.failure.unwrap_or(if post {"post-option"}else{"ok"}));
    let armed=controller.gpc_changed || controller.memory_changed;
    if !outcome.full && armed {TRIPPED.store(true,Ordering::Release);}
    *CONTROLLER.lock()=Some(controller);
    ARMED.store(armed,Ordering::Release);
}
fn guard_thread(base:usize) {
    // As with the existing timer-sleeping kernel workers, enable preemption
    // before sampling and sleeping. This worker is outside interrupt paths.
    unsafe {crate::arch_enable_interrupts();}
    let mut guard=policy::Guard::new();
    let mut logged=None;
    loop {
        let t=read_temperature(base);
        let trip=guard.sample(t);
        if trip {TRIPPED.store(true,Ordering::Release);}
        // A startup fallback with an unconfirmed restore also needs retrying.
        if TRIPPED.load(Ordering::Acquire) {guard.sample(Err("startup fallback"));}
        let now=now_ns();
        if logged!=Some(t) || trip {
            crate::serial_println!("GT710_CLOCK_TEMPERATURE monotonic_ns={} celsius={} tripped={}",now,t.unwrap_or(u32::MAX),guard.tripped());
            logged=Some(t);
        }
        if guard.critical(t) {
            HALTED.store(true,Ordering::Release);
            crate::gpu::panic::native_reset();
            // Existing reset paths stop runlists before disabling PGRAPH/PFIFO
            // through PMC_ENABLE. Keep them disabled at the critical threshold.
            let r=Registers(base);
            r.mask(0x2630,u32::MAX,u32::MAX);
            r.mask(0x200,0x1100,0);
            crate::serial_println!("GT710_CLOCK_CRITICAL temperature_c={} engines_disabled=true",t.unwrap_or(u32::MAX));
        }
        if guard.retry_due(now,ARMED.load(Ordering::Acquire)) {
            // Only this worker uses the controller after initialization. Its
            // waits must not pin preemption or stop the terminal's canvas.
            let controller={CONTROLLER.lock().take()};
            if let Some(mut c)=controller {
                let gpc=c.restore_gpc();
                let memory=c.restore_memory();
                let restored=gpc.is_ok() && memory.is_ok();
                if restored {ARMED.store(false,Ordering::Release);}
                crate::serial_println!("GT710_CLOCK_GUARD trip_c=85 temperature_c={} gpc_restore={} memory_restore={} armed={}",
                    t.unwrap_or(u32::MAX),gpc.err().unwrap_or("ok"),memory.err().unwrap_or("ok"),!restored);
                c.record(false,"thermal-guard-latched");
                *CONTROLLER.lock()=Some(c);
            }
            guard.restore_finished(now_ns());
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
