//! Suite-only root probes; no addresses, methods or mappings cross this interface.
//! The service owns channels and backing independently of the waiting process.
use super::*;
use core::sync::atomic::AtomicBool;
use crate::task::{waitqueue::{WaitQueueHead,PrepareOutcome},thread::ThreadState};
static SESSION:spin::Mutex<Option<Session>>=spin::Mutex::new(None);
static RECOVERY_STARTED:AtomicBool=AtomicBool::new(false);
static RECOVERY_READY:AtomicBool=AtomicBool::new(false);
static RECOVERY_RESULT:spin::Mutex<Option<Result<String,&'static str>>>=spin::Mutex::new(None);
static RECOVERY_WAITERS:WaitQueueHead=WaitQueueHead::new();
static WAITING:AtomicBool=AtomicBool::new(false);
static WAITERS:WaitQueueHead=WaitQueueHead::new();
struct Work {a:Channel,b:Channel,ticket:Option<Ticket>,request:Request,seed:u32,baseline:memory::allocator::Stats,old:[u32;3],userd:Allocation,engine:Engine}
pub(super) struct Session {pub(super) layout:memory::Layout,pub(super) registers:usize,pub(super) bar:usize,pub(super) bi:u64,pub(super) bd:u64,pub(super) pte:u64,pub(super) total:u64,work:Option<Work>}
pub(crate) fn retain(layout:memory::Layout,regs:usize,bar:usize)->Result<(),&'static str> {
    let native=layout.objects[4].range.start;
    let s=layout.allocator.stats();
    *SESSION.lock()=Some(Session{layout,registers:regs,bar,bi:native+0x2000,bd:native+0x3000,
        pte:native+0x4000+(SLOT/4096)*8,total:s.reserved+s.charged+s.free,work:None});
    Ok(())
}
impl Session {
    fn begin(&mut self)->Result<(),&'static str> {
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
