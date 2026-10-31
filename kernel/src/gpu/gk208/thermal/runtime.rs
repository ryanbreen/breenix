//! Probe-only temperature observations; sensor failures never abort startup.
use super::{Access,Write};
use crate::gk208::Registers;
use alloc::{format,vec::Vec};
struct State {base:usize,down:u32,setup:Result<(),&'static str>}
static STATE:spin::Mutex<Option<State>>=spin::Mutex::new(None);
struct Io {regs:Registers,raw:Option<u32>}
impl Access for Io {
    fn read(&mut self,a:u32)->u32 {
        let v=self.regs.read(a as usize);if a==0x20400 {self.raw=Some(v);}v
    }
    fn write(&mut self,a:u32,v:u32) {self.regs.write(a as usize,v)}
    fn record(&mut self,w:Write) {
        crate::serial_println!("GT710_SENSOR_WRITE {{\"address\":{},\"before\":{},\"written\":{},\"readback\":{},\"retained\":{}}}",w.address,w.before,w.written,w.readback,w.retained);
    }
    fn sleep_ms(&mut self,ms:u32) {
        let start=crate::time::get_monotonic_time_ns();
        let begin=start.0*1_000_000_000+start.1;
        loop {
            let (s,n)=crate::time::get_monotonic_time_ns();
            if (s*1_000_000_000+n).saturating_sub(begin)>=u64::from(ms)*1_000_000 {break;}
            core::hint::spin_loop();
        }
    }
}
pub(in crate::gk208) fn initialize(regs:&Registers,rom:&[u8]) {
    let decoded=super::down_clock(rom);
    let down=decoded.unwrap_or(85);
    let setup=super::setup(&mut Io {regs:Registers(regs.0),raw:None});
    crate::serial_println!("GT710_SENSOR_SETUP status={} down_clock_c={} threshold_source={}",
        setup.err().unwrap_or("ok"),down,if decoded.is_ok() {"vbios"}else{"fallback"});
    *STATE.lock()=Some(State {base:regs.0,down,setup});
}
pub(in crate::gk208) fn read_status()->Vec<u8> {
    let state=STATE.lock();
    let Some(s)=state.as_ref() else {return b"GT710_SENSOR celsius=null raw=null down_clock=85 status=not-initialized\n".to_vec();};
    let mut io=Io {regs:Registers(s.base),raw:None};
    let value=s.setup.and_then(|()|super::temperature(&mut io));
    let c=value.map_or_else(|_|alloc::string::String::from("null"),|v|format!("{v}"));
    let raw=io.raw.map_or_else(||alloc::string::String::from("null"),|v|format!("{v}"));
    format!("GT710_SENSOR celsius={c} raw={raw} down_clock={} status={}\n",s.down,value.err().unwrap_or("ok")).into_bytes()
}
