/*
 * Copyright 2012 Red Hat Inc.
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
//! M5.1's physical inventory probe. Fault registers are read-only adaptations
//! from MIT nouveau v6.8 nvkm/engine/fifo/gf100.c (see THIRD_PARTY_NOTICES).
//! BAR attachment uses the existing checked M4 small-page implementation.
use super::{memory::{self, allocator::{Kind, Stats}, Range}, modeset, Registers};
use crate::{arch_impl::x86_64::timer, drivers::pci};
use alloc::{format, string::String, vec, vec::Vec};
use core::fmt::Write;

fn stats(s:Stats)->String {
    format!("{{\"reserved\":{},\"payload\":{},\"guards\":{},\"charged\":{},\"display\":{},\"engine\":{},\"probe\":{},\"free\":{},\"allocations\":{}}}",
        s.reserved,s.payload,s.guards,s.charged,s.display,s.engine,s.probe,s.free,s.allocations)
}
fn pattern(address:u64,seed:u32,direction:usize)->u32 {
    let mut x=(address/4) as u32 ^ seed ^ 0xa511e9b3u32.wrapping_mul(direction as u32);
    x^=x>>16;x=x.wrapping_mul(0x7feb352d);x^=x>>15;x=x.wrapping_mul(0x846ca68b);x^(x>>16)
}
fn digest(regs:&mut Registers,total:u64,ranges:&[Range])->Result<(u64,String),&'static str> {
    let mut hash=crate::block::install::sha256::Sha256::new();
    let mut p=memory::Pramin::new(regs,total);let mut count=0;
    for r in ranges {
        for start in (r.start..r.end().ok_or("snapshot overflow")?).step_by(1<<20) {
            let size=(r.end().unwrap()-start).min(1<<20);
            hash.update(&p.bytes(Range{start,size})?);count+=size;
        }
    }
    Ok((count,super::hex(&hash.finalize())))
}
fn words(regs:&mut Registers,total:u64,address:u64,words:&[u32])->Result<(),&'static str> {
    let mut p=memory::Pramin::new(regs,total);
    for (i,word) in words.iter().enumerate() {p.write(address+i as u64*4,*word)?;}
    Ok(())
}
fn faults(regs:&mut Registers)->(u32,String) {
    let mask=regs.read(0x259c);let mut records=String::new();
    for unit in 0..32 {
        if mask&(1<<unit)==0 {continue;}
        if !records.is_empty(){records.push(',');}
        let r=0x2800+unit*16;
        let _=write!(records,"[{}, {}, {}, {}, {}]",unit,regs.read(r),regs.read(r+4),regs.read(r+8),regs.read(r+12));
    }
    (mask,records)
}
pub fn probe(regs:&mut Registers,device:&pci::Device,layout:&mut memory::Layout,console:&super::ConsoleSurface)->Result<(),&'static str> {
    let baseline=layout.allocator.stats();
    let total=baseline.reserved+baseline.charged+baseline.free;
    let seed=(timer::rdtsc() as u32)|1;
    let bar_before=regs.read(0x1704);let pramin_before=regs.read(0x1700);
    let (faults_before,records)=faults(regs);
    crate::serial_println!("GT710_VRAM_FAULT before={} records=[{}]",faults_before,records);
    if faults_before!=0 {return Err("preexisting GPU memory fault");}
    let mut protected=layout.allocator.reserved().to_vec();
    protected.extend(layout.objects.iter().map(memory::Object::guarded));
    crate::serial_println!("GT710_VRAM_STAGE snapshot-reserved-before bytes={}",protected.iter().map(|r|r.size).sum::<u64>());
    let (protected_bytes,protected_before)=digest(regs,total,&protected)?;
    let a=layout.allocator.allocate((1<<20)+4096,4096,Kind::Probe)?;
    let b=layout.allocator.allocate(12288,4096,Kind::Probe)?;
    let tables=layout.allocator.allocate(65536,4096,Kind::Engine)?;
    let peak=layout.allocator.stats();
    let mut guards=Vec::new();
    for allocation in [a,b,tables] {
        guards.extend([Range{start:allocation.range.start-4096,size:4096},Range{start:allocation.range.end().unwrap(),size:4096}]);
    }
    let (scratch_guard_bytes,scratch_guard_before)=digest(regs,total,&guards)?;
    let instance=tables.range.start;let directory=instance+4096;let table=instance+8192;
    let surface=layout.objects[0].range;
    let (_,displacement,_)=modeset::bar1_view(surface.start,surface.size,device.bars[1].size,17)?;
    let pointer=console.pointer-displacement; // Reuse the existing WC alias, never add a UC alias.
    // Never clear or reuse POST storage. All writes below target charged ranges.
    words(regs,total,instance+0x200,&[directory as u32,0,(modeset::BAR1_WINDOW-1) as u32,0])?;
    let ranges=[a.range,b.range];
    let mut view_hashes:Vec<[String;2]>=vec![[String::new(),String::new()];3];
    let mut table_hashes:Vec<String>=vec![String::new();3];
    let result=(|| {
        // Fill both physical allocations before reading either view.
        {let mut p=memory::Pramin::new(regs,total);
        for r in ranges {for address in (r.start..r.end().unwrap()).step_by(4) {p.write(address,pattern(address,seed,0))?;}}}
        for direction in 0..2 {
            for (step,index) in [0,1,0].into_iter().enumerate() {
                let r=ranges[index];let (pde,ptes)=modeset::bar1_small_pages(r.start,r.size,table)?;
                words(regs,total,directory,&pde)?;
                {let mut p=memory::Pramin::new(regs,total);
                for (i,pte) in ptes.iter().enumerate() {
                    p.write(table+i as u64*8,*pte as u32)?;p.write(table+i as u64*8+4,(*pte>>32) as u32)?;
                }}
                modeset::activate_bar1(regs,instance,directory)?;
                let mut p=memory::Pramin::new(regs,total);
                let bind=p.bytes(Range{start:instance+0x200,size:16})?;
                let expected=[directory as u32,0,(modeset::BAR1_WINDOW-1) as u32,0];
                if bind!=expected.iter().flat_map(|w|w.to_le_bytes()).collect::<Vec<_>>() {return Err("BAR instance read-back mismatch");}
                if p.bytes(Range{start:directory,size:8})?!=pde.iter().flat_map(|w|w.to_le_bytes()).collect::<Vec<_>>() {return Err("BAR directory read-back mismatch");}
                let mut actual=Vec::new();
                for (i,pte) in ptes.iter().enumerate() {
                    let bytes=p.bytes(Range{start:table+i as u64*8,size:8})?;
                    if bytes!=pte.to_le_bytes() {return Err("BAR small-page read-back mismatch");}
                    actual.extend_from_slice(&bytes);
                }
                table_hashes[step]=super::hash(&actual);
                drop(p);
                if direction==1 {
                    for offset in (0..r.size).step_by(4) {
                        // SAFETY: the bounded BAR view is installed above; aligned MMIO.
                        unsafe {((pointer+offset as usize) as *mut u32).write_volatile(pattern(r.start+offset,seed,1));}
                    }
                    // SAFETY: x86 WC stores must drain before BAR flush or a new binding.
                    unsafe {core::arch::asm!("sfence",options(nostack,preserves_flags));}
                    modeset::bar_flush(regs)?;
                }
                let mut digest=crate::block::install::sha256::Sha256::new();
                let mut p=memory::Pramin::new(regs,total);
                for offset in (0..r.size).step_by(4) {
                    let expected=pattern(r.start+offset,seed,direction);
                    let actual=if direction==0 {
                        // SAFETY: bounded and aligned installed BAR view.
                        unsafe {((pointer+offset as usize) as *const u32).read_volatile()}
                    } else {p.read(r.start+offset)?};
                    if actual!=expected {return Err("independent PRAMIN/BAR nonce mismatch");}
                    digest.update(&actual.to_le_bytes());
                }
                view_hashes[step][direction]=super::hex(&digest.finalize());
            }
        }
        // A final complete physical check also catches a later BAR window
        // corrupting the other allocation after its immediate read-back.
        let mut p=memory::Pramin::new(regs,total);
        for r in ranges {for address in (r.start..r.end().unwrap()).step_by(4) {
            if p.read(address)?!=pattern(address,seed,1) {return Err("BAR window aliased another allocation");}
        }}
        Ok(())
    })();
    // Restore the already-owned display instance and invalidate before freeing.
    let display_directory=layout.objects[4].range.start+0x3000;
    modeset::activate_bar1(regs,u64::from(bar_before&0x0fffffff)<<12,display_directory)?;
    if regs.read(0x1704)!=bar_before {return Err("display BAR binding not restored");}
    result?;
    crate::serial_println!("GT710_VRAM_STAGE snapshot-reserved-after");
    let (_,protected_after)=digest(regs,total,&protected)?;
    if protected_after!=protected_before {return Err("reserved VRAM or retained display allocation changed");}
    let (faults_after,records)=faults(regs);
    if faults_after!=0 {crate::serial_println!("GT710_VRAM_FAULT after={} records=[{}]",faults_after,records);return Err("GPU memory fault during aperture probe");}
    let (_,scratch_guard_after)=digest(regs,total,&guards)?;
    if scratch_guard_after!=scratch_guard_before {return Err("VRAM scratch guard changed");}
    layout.allocator.release(a)?;layout.allocator.release(b)?;layout.allocator.release(tables)?;
    let after=layout.allocator.stats();
    if after!=baseline {return Err("VRAM probe failed to reclaim charges");}
    let mut views=String::new();
    for (step,index) in [0,1,0].into_iter().enumerate() {
        if step!=0 {views.push(',');}
        let _=write!(views,"{{\"index\":{},\"instance\":{},\"directory\":{},\"table\":{},\"pde\":[0,{}],\"ptes_sha256\":\"{}\",\"sha256\":[\"{}\",\"{}\"]}}",
            index,instance,directory,table,(table>>8) as u32|1,table_hashes[step],view_hashes[step][0],view_hashes[step][1]);
    }
    crate::serial_println!("GT710_VRAM {{\"schema\":1,\"phase\":\"inventory\",\"bdf\":\"{:02x}:{:02x}.{}\",\"total\":{},\"aperture\":{},\"seed\":{},\"baseline\":{},\"peak\":{},\"after\":{},\"scratch\":[[{},{}],[{},{}]],\"tables\":[{},{}],\"views\":[{}],\"protected_bytes\":{},\"protected_before\":\"{}\",\"protected_after\":\"{}\",\"bar_before\":{},\"bar_after\":{},\"pramin_before\":{},\"pramin_after\":{},\"faults_before\":{},\"faults_after\":{},\"fault_records\":[{}],\"translation\":\"pending-m6\",\"scratch_guard_bytes\":{},\"scratch_guard_before\":\"{}\",\"scratch_guard_after\":\"{}\"}}",
        device.bus,device.device,device.function,total,device.bars[1].size,seed,stats(baseline),stats(peak),stats(after),
        a.range.start,a.range.size,b.range.start,b.range.size,instance,tables.range.size,views,protected_bytes,protected_before,protected_after,
        bar_before,regs.read(0x1704),pramin_before,regs.read(0x1700),faults_before,faults_after,records,scratch_guard_bytes,scratch_guard_before,scratch_guard_after);
    Ok(())
}
