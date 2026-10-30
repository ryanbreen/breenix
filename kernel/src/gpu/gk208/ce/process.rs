//! Suite-only root probes; no addresses, methods or mappings cross this interface.
//! The service owns channels and backing independently of the waiting process.
use super::*;
use core::sync::atomic::AtomicBool;
use crate::task::{waitqueue::{WaitQueueHead,PrepareOutcome},thread::ThreadState};
static SESSION:super::super::lock::Mutex<Option<Session>>=super::super::lock::Mutex::new(None);
// Sticky through failed startup too: cleanup may retain channels/backing.
// Keep this visible while initialize_core temporarily takes SESSION.
static CORE_STARTED:AtomicBool=AtomicBool::new(false);
static RECOVERY_STARTED:AtomicBool=AtomicBool::new(false);
static RECOVERY_READY:AtomicBool=AtomicBool::new(false);
static RECOVERY_RESULT:spin::Mutex<Option<Result<String,&'static str>>>=spin::Mutex::new(None);
static RECOVERY_WAITERS:WaitQueueHead=WaitQueueHead::new();
static WAITING:AtomicBool=AtomicBool::new(false);
static WAITERS:WaitQueueHead=WaitQueueHead::new();
struct Work {a:Channel,b:Channel,ticket:Option<Ticket>,request:Request,seed:u32,baseline:memory::allocator::Stats,old:[u32;3],userd:Allocation,engine:Engine}
pub(super) struct Session {pub(super) layout:memory::Layout,pub(super) registers:usize,pub(super) bar:usize,pub(super) bi:u64,pub(super) bd:u64,pub(super) pte:u64,pub(super) total:u64,console:super::super::ConsoleSurface,mode:u32,evo:super::super::native::Evo,scanout:Vec<super::super::resource::Hold>,work:Option<Work>,resources:super::super::resource::Resources<crate::gpu::dma::Pages>}
pub(in crate::gk208) fn test_ownership()->Result<(),&'static str> {
    if CORE_STARTED.load(Ordering::Acquire) {return Err("native GPU core owns FIFO");}
    Ok(())
}
pub(crate) fn retain(mut layout:memory::Layout,regs:usize,bar:usize,console:super::super::ConsoleSurface,mut evo:super::super::native::Evo)->Result<super::super::ConsoleSurface,&'static str> {
    test_ownership()?;
    let device=crate::drivers::pci::find_device(0x10de,0x128b).ok_or("native PCI device absent")?;
    let console=super::super::native::logical_console(&mut Registers(regs),&mut layout,
        &mut evo,console,bar,device.bars[1].size)?;
    let native=layout.objects[4].range.start;
    let table=layout.objects.iter().find(|o|o.name=="bar1-table")
        .ok_or("logical console BAR table absent")?.range.start;
    let s=layout.allocator.stats();
    *SESSION.lock()=Some(Session{layout,registers:regs,bar,bi:native+0x2000,bd:native+0x3000,
        pte:table+(SLOT/4096)*8,total:s.reserved+s.charged+s.free,console,mode:1,evo,scanout:Vec::new(),work:None,resources:super::super::resource::Resources::new()});
    Ok(console)
}
pub(in crate::gk208) fn initialize_core()->Result<(),&'static str> {
    let mut s={
        let mut owner=SESSION.lock();
        let s=owner.as_mut().ok_or("native display/allocator unavailable")?;
        if CORE_STARTED.load(Ordering::Acquire) {return Err("native core startup already attempted");}
        if s.work.is_some() {return Err("native CE work still owned");}
        CORE_STARTED.store(true,Ordering::Release);
        owner.take().unwrap()
    };
    let result=(|| {
        let device=crate::drivers::pci::find_device(0x10de,0x128b).ok_or("native PCI device absent")?;
        super::super::gr::initialize(&mut Registers(s.registers),&device,&mut s.layout,&s.console)
    })();
    // Builders borrow SESSION. Restore it before probing, without holding its
    // lock across device submission/reaping or storage retirement.
    *SESSION.lock()=Some(s);
    let mut native = result?;
    crate::gpu::dev::select_gk208();
    if let Err(error) = super::super::caps::run(&mut native) {
        // Uncertain publication retains channel/request ownership. A caller's
        // startup error does not establish DMA cessation.
        core::mem::forget(native);
        return Err(error);
    }
    crate::gpu::install_native(native)
}

impl Session {
    fn begin(&mut self)->Result<(),&'static str> {
        test_ownership()?;
        if self.work.is_some() {return Err("native wait already owned");}
        let mut regs=Registers(self.registers);
        ensure_unbound(&mut regs, 0)?;
        ensure_unbound(&mut regs, 1)?;
        let e=discover(&mut regs)?;
        let old=[regs.read(0x2254),regs.read(0x2140),regs.read(0x204)];
        regs.write(0x2140,0);regs.write(e.base+0x904,0);
        regs.mask(0x204,1<<e.pbdma,1<<e.pbdma);
        regs.write(0x2254,0x10000000|(SLOT>>12) as u32);
        let baseline=self.layout.allocator.stats();let status=system_dma::translation_status()?;
        let userd=self.layout.allocator.allocate(4096,4096,Kind::Engine)?;
        let mut a=Channel::new(&mut regs,&mut self.layout,self.total,0,&status,userd)?;
        let mut b=Channel::new(&mut regs,&mut self.layout,self.total,1,&status,userd)?;
        let seed=timer::rdtsc() as u32;
        for (i,c) in [&mut a,&mut b].into_iter().enumerate() {
            let data:Vec<_>=(0..SIZE).map(|n|pattern(n,if i==0 {seed}else{!seed})).collect();
            fill(&mut regs,self.total,c.source.range.start,&data)?;
            fill(&mut regs,self.total,c.dest.range.start,&[0xcc;SIZE])?;
        }
        modeset::bar_flush(&mut regs)?;
        activate_pair(&mut regs,self.total,&e,self.bi,self.bd,self.pte,&mut a,&mut b)?;
        let ticket=a.context.hold(Consumer::Queued)?;
        let seq=a.queue.next()?;let command=gated_copy(&a,seq)?;
        a.push(&mut regs,self.total,self.bar,&command,true)?;
        let request=a.request(seq,a.dest.range.start,1);
        self.work=Some(Work{a,b,ticket:Some(ticket),request,seed,baseline,old,userd,engine:e});
        // Copy on the second channel while the first waits on its private gate.
        let w=self.work.as_mut().unwrap();
        w.b.execute(&mut regs,self.total,&w.engine,self.bar,rect(SOURCE,DEST,8192,1,8192,8192))?;
        if w.a.completed(&mut regs,self.total,&w.engine,w.request,0)?.0.is_some() {return Err("gated copy ran before release");}
        Ok(())
    }
    fn status(&mut self)->Result<String,&'static str> {
        test_ownership()?;
        let Some(w)=self.work.as_mut() else {return Ok(String::from("ready\n"));};
        let mut regs=Registers(self.registers);let e=&w.engine;
        let completed=w.a.completed(&mut regs,self.total,&e,w.request,0)?.0.is_some();
        let held=w.a.context.holds()[0];
        // Refused releases retain the measured charges and mapping pins.
        let before=self.layout.allocator.stats();
        if self.layout.allocator.release(w.a.commands).is_ok() || self.layout.allocator.release(w.a.dest).is_ok()
            || self.layout.allocator.stats()!=before {return Err("waiting CE storage was released");}
        Ok(format!("waiting held={} completed={}\n",held,u32::from(completed)))
    }
    fn drain(&mut self)->Result<String,&'static str> {
        test_ownership()?;
        let w=self.work.as_mut().ok_or("native waiter absent")?;
        let mut regs=Registers(self.registers);let e=&w.engine;
        if w.a.completed(&mut regs,self.total,&e,w.request,0)?.0.is_some() {return Err("native wait already completed");}
        words(&mut regs,self.total,w.a.commands.range.start+9216,&[1])?;
        modeset::bar_flush(&mut regs)?;
        w.a.completed(&mut regs,self.total,&e,w.request,2_000_000)?.0.ok_or("deferred release missing")?;
        w.a.queue.complete(w.request.sequence)?;
        unsafe {w.a.context.finish_device(w.ticket.take().ok_or("lost deferred hold")?)?;}
        for poison in [0x55,0xaa] {
            for (i,c) in [&mut w.a,&mut w.b].into_iter().enumerate() {
                system_fill(c,poison);
                c.execute(&mut regs,self.total,&e,self.bar,rect(DEST,SYSTEM,8192,1,8192,8192))?;
                let bytes=system_bytes(c);
                if bytes.iter().enumerate().any(|(n,v)|*v!=pattern(n,if i==0 {w.seed}else{!w.seed})) {
                    return Err("killed wait or surviving channel full compare failed");
                }
            }
        }
        w.a.drain(&mut regs,&e)?;w.b.drain(&mut regs,&e)?;
        words(&mut regs,self.total,self.pte,&[0,0])?;modeset::activate_bar1(&mut regs,self.bi,self.bd)?;
        let w=self.work.take().unwrap();
        w.a.retire(&mut regs,&mut self.layout,self.total)?;w.b.retire(&mut regs,&mut self.layout,self.total)?;self.layout.allocator.release(w.userd)?;
        if self.layout.allocator.stats()!=w.baseline {return Err("deferred destruction baseline differs");}
        regs.write(0x2254,w.old[0]);regs.write(0x2140,w.old[1]);regs.write(0x204,w.old[2]);
        Ok(String::from("drained compares=2 survivor=1 baseline=1\n"))
    }
}
fn recovery_read()->Result<Vec<u8>,i32> {
    if !RECOVERY_STARTED.load(Ordering::Acquire) && SESSION.lock().is_none() {return Ok(b"unavailable\n".to_vec());}
    if !RECOVERY_STARTED.swap(true,Ordering::AcqRel) {
        let started=crate::task::kthread::kthread_run(|| {
            let owned={SESSION.lock().take()};
            let result=if let Some(mut s)=owned {
                let result=if s.work.is_some() {Err("native deferred work still owned")} else {super::recovery::probe(&mut s)};
                *SESSION.lock()=Some(s);result
            } else {Err("native recovery owner absent")};
            *RECOVERY_RESULT.lock()=Some(result);
            RECOVERY_READY.store(true,Ordering::Release);
            RECOVERY_WAITERS.wake_up();
        },"kgk208");
        if started.is_err() {
            *RECOVERY_RESULT.lock()=Some(Err("native recovery worker creation failed"));
            RECOVERY_READY.store(true,Ordering::Release);
            RECOVERY_WAITERS.wake_up();
            return Err(-12);
        }
    }
    loop {
        if RECOVERY_READY.load(Ordering::Acquire) {
            return Ok(match RECOVERY_RESULT.lock().as_ref().ok_or(-5)? {Ok(s)=>s.as_bytes().to_vec(),Err(why)=>format!("GT710_RECOVERY_ERROR {}\n",why).into_bytes()});
        }
        let outcome=RECOVERY_WAITERS.prepare_to_wait_checked(ThreadState::BlockedOnIO,None,||!RECOVERY_READY.load(Ordering::Acquire));
        crate::syscall::blocking_io::wait_prepared(&RECOVERY_WAITERS,outcome).map_err(|e|-e)?;
    }
}
pub(crate) fn proc_read(op:u8)->Result<Vec<u8>,i32> {
    if !crate::test_control::enabled() {return Err(-38);}
    if op==3 {return recovery_read();}
    if op==1 {
        {
            let mut session=SESSION.lock();let s=session.as_mut().ok_or(-19)?;
            s.begin().inspect_err(|e|crate::serial_println!("GT710_PROCESS_ERROR begin {}",e)).map_err(|_|-5)?;
        }
        // The caller sleeps after releasing its device and allocator locks.
        let start=timer::rdtsc();
        let (seconds,nanos)=crate::time::timer::get_monotonic_time_ns();
        let deadline=seconds.saturating_mul(1_000_000_000).saturating_add(nanos).saturating_add(2_000_000_000);
        loop {
            if elapsed(start)>=2_000_000 {return Err(-5);}
            if crate::syscall::check_signals_for_wait().is_some() {return Err(-4);}
            {
                let mut session=SESSION.lock();let s=session.as_mut().ok_or(-19)?;
                let w=s.work.as_mut().ok_or(-5)?;let mut regs=Registers(s.registers);
                if w.a.completed(&mut regs,s.total,&w.engine,w.request,0).map_err(|_|-5)?.0.is_some() {
                    return Ok(b"completed\n".to_vec());
                }
            }
            let (seconds,nanos)=crate::time::timer::get_monotonic_time_ns();
            let wake=seconds.saturating_mul(1_000_000_000).saturating_add(nanos).saturating_add(10_000_000).min(deadline);
            let outcome=WAITERS.prepare_to_wait_checked(ThreadState::BlockedOnIO,Some(wake),||true);
            if matches!(outcome,PrepareOutcome::Queued) {WAITING.store(true,Ordering::Release);}
            crate::syscall::blocking_io::wait_prepared(&WAITERS,outcome).map_err(|e|-e)?;
            if matches!(outcome,PrepareOutcome::Mismatch) {continue;}
        }
    }
    let mut session=SESSION.lock();
    let Some(s)=session.as_mut() else {return Ok(b"unavailable\n".to_vec());};
    if op==2 {return s.drain().inspect_err(|e|crate::serial_println!("GT710_PROCESS_ERROR drain {}",e)).map(String::into_bytes).map_err(|_|-5);}
    if !WAITING.load(Ordering::Acquire) {return Ok(b"ready\n".to_vec());}
    s.status().map(String::into_bytes).map_err(|_|-5)
}

/// Borrow the allocator retained by the display/CE owner.
/// The session lock serializes resource changes with CE ownership.
pub(in crate::gk208) fn with_resources<R>(
    f: impl FnOnce(&mut super::super::resource::Resources<crate::gpu::dma::Pages>,
        &mut memory::allocator::Allocator, &mut VmIo<'_>) -> Result<R, &'static str>,
) -> Result<R, &'static str> {
    let mut owner = SESSION.lock();
    let s = owner.as_mut().ok_or("native resource allocator unavailable")?;
    if !CORE_STARTED.load(Ordering::Acquire) || s.work.is_some() {
        return Err("native resource allocator not owned by core");
    }
    let before = s.resources.vram_bytes();
    let result = f(&mut s.resources, &mut s.layout.allocator,
        &mut VmIo { regs: &mut Registers(s.registers), total: s.total });
    let after = s.resources.vram_bytes();
    crate::tracing::providers::gpu::gauge(&crate::tracing::providers::gpu::GPU_VRAM_BYTES,
        after as i64 - before as i64);
    result
}

/// Observe the existing native console, independently of the shared renderer's
/// resources. Its identifier is reserved, rather than its physical address.
pub(crate) fn console_state() -> Option<(gpu_abi::Mode, u64, bool, u64)> {
    let owner = SESSION.lock();
    let session=owner.as_ref()?;
    let s = &session.console;
    let bound = super::super::console_binding(s).is_ok();
    let writes = crate::firmware_writes::count().checked_sub(s.firmware_baseline)?;
    let (width,height,scale)=if session.mode==0 {(1280,800,1)} else {(2560,1600,2)};
    Some((gpu_abi::Mode { width, height, scale, reserved: 0 }, u64::from(u32::MAX), bound, writes))
}

pub(in crate::gk208) fn console_allocation() -> Option<(super::super::ConsoleSurface, Allocation)> {
    let owner = SESSION.lock();
    let s = owner.as_ref()?;
    let allocation = s.layout.objects.iter().find(|o| o.range.start == s.console.vram)?.allocation;
    Some((s.console, allocation))
}

/// Invoked only after the adapter's in-order native release has completed.
/// Retain the active target independently of requests until another surface is
/// confirmed. An uncertain update retains its request hold in the adapter.
pub(in crate::gk208) fn present(surface:super::super::present::Surface)
    -> Result<(), &'static str> {
    let mut owner=SESSION.lock();
    let s=owner.as_mut().ok_or("native display owner absent")?;
    let hold=s.resources.hold(surface.resource)?;
    // An error can follow an actual latch. Keep possible display readers
    // pinned through GR reset; only a confirmed later update can release it.
    s.scanout.push(hold);
    let receipt=super::super::native::present(&mut s.evo,&mut Registers(s.registers),surface)?;
    if receipt.notifier&1!=1 || receipt.active!=receipt.armed {
        return Err("native present receipt mismatch");
    }
    s.mode=if surface.width==1280 {0} else {1};
    let active=s.scanout.pop().unwrap();
    s.scanout.clear();
    s.scanout.push(active);
    Ok(())
}
/// Serialize a logical mode switch with client display readers through the
/// shared ownership transition. No core spin lock is held across EVO waits.
pub(crate) fn set_mode(index:u32)->Result<(), &'static str> {
    if index>1 {return Err("invalid native logical mode");}
    let mut owner=SESSION.lock();
    let s=owner.as_mut().ok_or("native display owner absent")?;
    let surface=if index==1 {
        let c=s.console;
        super::super::present::Surface{resource:u32::MAX,address:c.vram,
            width:2560,height:1600,pitch:10240,block_height:0,format:0,dma:1}
    } else {
        let range=s.layout.objects[1].range;
        if range.size<1280*800*4 {return Err("native low-mode surface too small");}
        super::super::present::Surface{resource:u32::MAX,address:range.start,
            width:1280,height:800,pitch:5120,block_height:0,format:0,dma:1}
    };
    super::super::native::present(&mut s.evo,&mut Registers(s.registers),surface)?;
    s.mode=index;
    s.scanout.clear();
    Ok(())
}
pub(in crate::gk208) fn mode_extent()->Option<(u32,u32)> {
    let owner=SESSION.lock();
    Some(if owner.as_ref()?.mode==0 {(1280,800)} else {(2560,1600)})
}

/// Prepare a disjoint terminal command packet while the display owner is live.
pub(in crate::gk208) fn terminal_display(surface: super::super::present::Surface)
    -> Result<super::super::emergency::Display, &'static str> {
    let owner = SESSION.lock();
    let s = owner.as_ref().ok_or("terminal display owner absent")?;
    s.evo.terminal_display(s.registers, surface)
}
