/*
 * Copyright 2012 Red Hat Inc.
 * Copyright 2014 Martin Peres
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
//! C1 startup measurements. No clock/PLL/voltage/timing writes.
use alloc::{format, string::String, vec::Vec};
use super::Registers;
use crate::drivers::pci;

fn pairs(raw:&[(u32,u32)])->String { raw.iter().map(|(a,v)|format!("[{a},{v}]")).collect::<Vec<_>>().join(",") }
fn reading(v:Result<u32,&'static str>)->String {
    match v {Ok(v)=>format!("{{\"value\":{v},\"status\":\"ok\"}}"),Err(e)=>format!("{{\"value\":null,\"status\":\"{e}\"}}")}
}
fn publish(kind:&str,row:String)->Result<(),&'static str> {
    let digest=super::hash(row.as_bytes());let bytes=row.len();
    super::bulk::line(row)?;
    crate::serial_println!("GT710_{}_BULK bytes={} sha256={}",kind,bytes,digest);Ok(())
}
fn pcie(device:&pci::Device)->Result<String,&'static str> {
    let read=|o|pci::pci_read_config_dword(device.bus,device.device,device.function,o);
    // Bounded conventional capability walk; configuration reads only.
    let mut at=(read(0x34)&0xfc) as u8;let mut seen=[false;64];let mut found=None;
    while at!=0 {
        if at<0x40 || seen[at as usize/4] {return Err("PCIe capability chain invalid");}
        seen[at as usize/4]=true;let cap=read(at);
        if cap&255==0x10 {found=Some(at);break;}
        at=((cap>>8)&0xfc) as u8;
    }
    let c=found.ok_or("PCIe capability absent")?;
    if c>0xdc {return Err("PCIe capability outside conventional config");}
    let flags=read(c)>>16;let dev=read(c+4);let link=read(c+12);let cs=read(c+16);
    let slot=if flags&0x100!=0 {Some(read(c+20))}else{None};
    crate::serial_println!("GT710_PCIE bdf={:02x}:{:02x}.{} cap={:#x} flags={:#x} device={:#x} link_cap={:#x} link_control={:#x} link_status={:#x} slot={}",
        device.bus,device.device,device.function,c,flags,dev,link,cs&65535,cs>>16,slot.map_or_else(||String::from("none"),|s|format!("{s:#x}")));
    Ok(format!("{{\"bdf\":\"{:02x}:{:02x}.{}\",\"cap_offset\":{},\"flags\":{},\"device_cap\":{},\"link_cap\":{},\"link_control\":{},\"link_status\":{},\"slot_cap\":{}}}",device.bus,device.device,device.function,c,flags,dev,link,cs&65535,cs>>16,slot.map_or_else(||String::from("null"),|s|format!("{s}"))))
}
pub(super) fn startup(regs:&mut Registers,device:&pci::Device)->Result<(),&'static str> {
    // The full 256 KiB PROM window is private bulk data (docs/gui/gt710-c1.md).
    let rom=super::rom::read_full(regs)?;
    super::c2::initialize(regs,&rom)?;
    let row=format!("GT710_VBIOS_V1 {{\"bytes\":{},\"sha256\":\"{}\",\"hex\":\"{}\"}}",rom.len(),super::hash(&rom),super::hex(&rom));
    publish("VBIOS",row)?;
    let (raw,domains)=super::clock::sample_domains(|a|regs.read(a as usize));
    for (n,f) in &domains {
        match f {Ok(v)=>crate::serial_println!("GT710_CLOCK domain={} khz={} status=ok",n,v),Err(e)=>crate::serial_println!("GT710_CLOCK domain={} khz=null status={}",n,e)}
    }
    let mut sensor_raw=Vec::new();
    let temp=super::sensor::temperature(|a| {
        let v=regs.read(a as usize);sensor_raw.push((a,v));v
    });
    crate::serial_println!("GT710_TEMPERATURE reading={} restored={}",reading(temp.value),temp.restored);
    let sensor_raw=pairs(&sensor_raw);
    let mut voltage_raw:Vec<_>=(0..32).map(|n|{let a=0xd610+n*4;(a,regs.read(a as usize))}).collect();
    for a in [0x20340,0x20344] {voltage_raw.push((a,regs.read(a as usize)));}
    let mut ports=alloc::vec![pcie(device)?];
    let bridges:Vec<_>=pci::get_devices().ok_or("PCI inventory absent")?.into_iter().filter(|d| {
        if d.raw_class!=6 || d.subclass!=4 {return false;}
        let buses=pci::pci_read_config_dword(d.bus,d.device,d.function,0x18);
        (buses>>8)&255==u32::from(device.bus)
    }).collect();
    if bridges.len()!=1 {return Err("GPU upstream port ambiguous or absent");}
    ports.push(pcie(&bridges[0])?);
    let ds=domains.iter().map(|(n,f)|format!("\"{}\":{}",n,reading(*f))).collect::<Vec<_>>().join(",");
    publish("C1",format!("GT710_CLOCK_C1 {{\"schema\":1,\"phase\":\"startup\",\"raw\":[{}],\"domains\":{{{}}},\"temperature\":{},\"sensor_restored\":{},\"sensor_raw\":[{}],\"sensor_writes\":[{}],\"voltage_raw\":[{}],\"pcie\":[{}]}}",pairs(&raw),ds,reading(temp.value),temp.restored,sensor_raw,"",pairs(&voltage_raw),ports.join(",")))?;
    Ok(())
}
