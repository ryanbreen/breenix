//! Probe-only descriptor and serialization for the C2 GPC controller.
use super::*;
use crate::gk208::{Registers,clock};
use alloc::{format,sync::Arc,vec::Vec};

struct State {base:usize,post:u32,limits:Limits,failed:bool,snapshot:Vec<(u32,u32)>,phase:u8}
static STATE:spin::Mutex<Option<State>>=spin::Mutex::new(None);
pub(in crate::gk208) fn initialize(regs:&Registers,rom:&[u8])->Result<(),&'static str> {
    let l=limits(rom)?;
    let post=clock::sample(|a|regs.read(a as usize)).gpc_khz?;
    if post!=810_000 {return Err("C2 requires measured POST GPC 810000");}
    let snapshot=observe::capture(&mut Io(Registers(regs.0)));
    print_snapshot("post-inventory",&snapshot);
    *STATE.lock()=Some(State {base:regs.0,post,limits:l,failed:false,snapshot,phase:0});Ok(())
}
struct Io(Registers);
impl Access for Io {
    fn read(&mut self,a:u32)->u32 {self.0.read(a as usize)}
    fn write(&mut self,a:u32,v:u32) {self.0.write(a as usize,v)}
    fn wait(&mut self,a:u32,mask:u32,value:u32)->Result<(),&'static str> {
        let start=crate::time::get_monotonic_time_ns();
        loop {
            if self.read(a)&mask==value {return Ok(());}
            let now=crate::time::get_monotonic_time_ns();
            if now.0.saturating_mul(1_000_000_000)+now.1-(start.0*1_000_000_000+start.1)>=2_000_000_000 {
                return Err("GPC switch/lock timeout");
            }
            core::hint::spin_loop();
        }
    }
}
impl observe::CounterAccess for Io {
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
fn measure_record(io:&mut Io,phase:&str)->Result<(),&'static str> {
    let m=crate::gk208::ce::with_resources(|_,_,_|observe::measure(io))?;
    crate::serial_println!("GT710_CLOCK_MEASURED {{\"phase\":\"{}\",\"domain\":\"gpc\",\"khz\":{},\"method\":\"gpc0-pre-counter-ptimer\",\"cycles\":{},\"ptimer_ns\":{},\"bracket_ns\":{},\"counter_restored\":true}}",
        phase,m.khz,m.cycles,m.ptimer_ns,m.bracket_ns);
    Ok(())
}
pub fn read_status()->Result<Vec<u8>,i32> {
    let state=STATE.lock();let s=state.as_ref().ok_or(-19)?;
    let sample=clock::sample(|a|Registers(s.base).read(a as usize));
    let khz=sample.gpc_khz.map_err(|_|-5)?;
    Ok(format!("GT710_CLOCK_READ {{\"domain\":\"gpc\",\"readback_khz\":{khz},\"post_khz\":{},\"status\":\"ok\"}}\n",s.post).into_bytes())
}
fn set(bytes:&[u8])->Result<(),&'static str> {
    let text=core::str::from_utf8(bytes).map_err(|_|"invalid request")?;
    let fields:Vec<_>=text.split_whitespace().collect();
    if fields.len()!=2 {return Err("request needs two fields");}
    let phase=match (fields[0],fields[1]) {
        ("observe","post")=>0,("gpc","540000")=>1,("restore","post")=>2,
        _=>return Err("C2b requires observe post, gpc 540000, restore post"),
    };
    let requested=if phase==1 {540_000}else{810_000};
    let mut state=STATE.lock();let s=state.as_mut().ok_or("C2 not initialized")?;
    if s.failed {return Err("C2 stopped after failure");}
    if phase!=s.phase {return Err("C2b phase out of order");}
    let mut io=Io(Registers(s.base));
    let result=(|| {
        if phase==0 {return Ok(());}
        if phase==2 {
            return crate::gk208::ce::with_resources(|_,_,_|observe::restore(&mut io,&s.snapshot));
        }
        if requested>s.post.min(810_000) || requested<540_000 {return Err("outside C2 POST envelope");}
        let refclk=clock::gpc_reference(|a|io.read(a))?;
        let vco=clock::gpc_vco(|a|io.read(a))?;
        let p=plan("gpc",requested,s.post,&s.limits,refclk,vco)?;
        if p.khz>s.post || p.khz>810_000 {return Err("calculated clock above POST");}
        let ctrl=io.read(0x137250);
        if ctrl&0x80000000!=0 && ctrl&0x3f3f!=0 {return Err("POST final divider not unity");}
        // The caller's final GPU WAIT precedes the write. Serialize with the
        // native CE/GR owner so no other publication crosses the source switch.
        crate::gk208::ce::with_resources(|_,_,_| program(&mut io,&p))?;
        Ok(())
    })();
    // Register-derived readback is retained as evidence, separate from the counter.
    let sample=clock::sample(|a|io.read(a));
    let readback=sample.gpc_khz;
    let step=clock::gpc_pll_step(|a|io.read(a)).unwrap_or(1);
    let result=result.and_then(|()| {
        if readback?.abs_diff(requested)>step {Err("readback-mismatch")}else{Ok(())}
    });
    let name=match phase {0=>"post",1=>"low",_=>"restored"};
    // Retain the actual register differences even when restoration refuses.
    let snapshots=snapshot_records(s,&mut io,name);
    let result=result.and(snapshots).and_then(|()|measure_record(&mut io,name));
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
