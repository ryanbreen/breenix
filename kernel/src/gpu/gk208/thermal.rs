/*
 * Copyright 2012 Red Hat Inc.
 * Copyright 2014 Martin Peres
 * Copyright 2012 Nouveau Community
 * Copyright 2018 Red Hat Inc.
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
 * 	    Martin Peres
 */
//! C3b sensor mechanism: MIT nouveau v6.8 therm/g84.c:29-51,
//! fuse/gf100.c:27-45; BIOS threshold lookup: bios/therm.c:27-60,75-106.
//! GK104 selects gf119 init and g84_temp_get (therm/gk104.c:93-102).
//! No clock, voltage, fan, alarm or calibration-coefficient writes.
#[derive(Clone, Copy, Debug)]
pub struct Write {
    pub address:u32, pub before:u32, pub written:u32, pub readback:u32, pub retained:bool, pub restoring:bool,
}
pub trait Access {
    fn read(&mut self,address:u32)->u32;
    fn write(&mut self,address:u32,value:u32);
    fn record(&mut self,write:Write);
    fn fuse_note(&mut self,result:Result<u32,&'static str>);
    fn sleep_ms(&mut self,milliseconds:u32);
}
fn write(io:&mut impl Access,address:u32,value:u32,retained:bool,restoring:bool)->u32 {
    let before=io.read(address);io.write(address,value);let readback=io.read(address);
    io.record(Write {address,before,written:value,readback,retained,restoring});readback
}
fn fuse(io:&mut impl Access)->Result<u32,&'static str> {
    let enable=io.read(0x22400);let access=io.read(0x21000);
    if [enable,access].iter().any(|v|*v==u32::MAX || *v==0xbadf1201) {return Err("fuse-access-unavailable");}
    write(io,0x22400,enable|0x800,false,false);
    write(io,0x21000,access|1,false,false);
    let value=io.read(0x212a8);
    // Restore both access words, including after a refused enable.
    let restored_access=write(io,0x21000,access,false,true);
    let restored_enable=write(io,0x22400,enable,false,true);
    if restored_access!=access || restored_enable!=enable {return Err("fuse-restore-failed");}
    Ok(value)
}
pub fn setup(io:&mut impl Access)->Result<(),&'static str> {
    let note=fuse(io);io.fuse_note(note);
    if note?!=1 {return Err("sensor-unavailable");}
    let a=io.read(0x20008);let b=io.read(0x2000c);
    if [a,b].iter().any(|v|*v==u32::MAX || *v==0xbadf1201) {return Err("sensor-control-unavailable");}
    // Nouveau leaves these controls in place for continuous hardware readings.
    let a=write(io,0x20008,(a&!0x80008000)|0x80000000,true,false);
    let b=write(io,0x2000c,b&!0x80000003,true,false);
    io.sleep_ms(20);
    if a&0x80008000!=0x80000000 || b&0x80000003!=0 {return Err("sensor-enable-failed");}
    Ok(())
}
pub fn temperature(io:&mut impl Access)->Result<u32,&'static str> {
    // G84+ is already hardware-calibrated: g84_temp_get returns this register
    // as integer Celsius, without applying the older NV40 BIOS slope/offset.
    if fuse(io)?!=1 {return Err("sensor-unavailable");}
    let raw=io.read(0x20400);
    if raw>i32::MAX as u32 {Err("sensor-read-error")}else{Ok(raw)}
}
#[cfg(any(feature="gt710_clock_probe",test))]
#[path="thermal/status.rs"] mod status;
#[cfg(any(feature="gt710_clock_probe",test))]
pub use status::read as status;
#[cfg(any(feature="gt710_clock_probe",test))]
fn bytes(r:&[u8],p:usize,n:usize)->Result<&[u8],&'static str> {
    r.get(p..p.checked_add(n).ok_or("thermal-table-overflow")?).ok_or("thermal-table-outside-ROM")
}
#[cfg(any(feature="gt710_clock_probe",test))]
fn word(r:&[u8],p:usize)->Result<usize,&'static str> {Ok(u16::from_le_bytes(bytes(r,p,2)?.try_into().unwrap()) as usize)}
#[cfg(any(feature="gt710_clock_probe",test))]
pub fn down_clock(r:&[u8])->Result<u32,&'static str> {
    let bit=r.windows(6).position(|s|s==b"\xff\xb8BIT\0").ok_or("thermal-BIT-missing")?;
    let h=bytes(r,bit,12)?;
    if h[8]!=12 || h[9]!=6 || h.iter().fold(0u8,|s,b|s.wrapping_add(*b))!=0 {return Err("thermal-BIT-header");}
    let mut pointer=None;
    for e in bytes(r,bit+12,h[10] as usize*6)?.chunks_exact(6).filter(|e|e[0]==b'P') {
        if pointer.is_some() {return Err("thermal-duplicate-BIT-P");}
        let data=bytes(r,word(e,4)?,word(e,2)?)?;
        let off=match e[1] {1=>12,2=>16,_=>return Err("thermal-BIT-P-version")};
        pointer=Some(u32::from_le_bytes(bytes(data,off,4)?.try_into().unwrap()) as usize);
    }
    let p=pointer.filter(|p|*p!=0).ok_or("thermal-table-missing")?;
    let h=bytes(r,p,4)?;
    if ![0x10,0x20,0x24].contains(&h[0]) || h[1]<4 || h[2]<3 {return Err("thermal-table-geometry");}
    let entries=bytes(r,p+h[1] as usize,h[2] as usize*h[3] as usize)?;
    let mut down=None;
    for e in entries.chunks_exact(h[2] as usize) {
        let value=u16::from_le_bytes([e[1],e[2]]);
        if e[0]==0 && (value as i16)>0 {break;}
        if e[0]==7 {down=Some(u32::from((value&0xff0)>>4));}
    }
    down.filter(|v|*v>0).ok_or("thermal-downclock-missing")
}
#[cfg(all(target_os="none", feature="gt710_clock_probe"))]
mod runtime;
#[cfg(all(target_os="none", feature="gt710_clock_probe"))]
pub(super) use runtime::{initialize,read_status};
