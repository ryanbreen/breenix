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
/*
 * Copyright 2005-2006 Erik Waling
 * Copyright 2006 Stephane Marchesin
 * Copyright 2007-2009 Stuart Bennett
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
 * THE AUTHORS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
 * WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF
 * OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
 * SOFTWARE.
 */
//! Probe-only GPC control. MIT nouveau Linux v6.8 e8f897f4afef0031fe618a8e94127a0934896aba:
//! clk/gk104.c:222-335,356-473 (idx 0 only), clk/pllgt215.c:31-100,
//! bios/pll.c:81-107,142-159,352-365. Original notices retained above.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub min_vco:u32, pub max_vco:u32, pub min_input:u32, pub max_input:u32,
    pub min_m:u32, pub max_m:u32, pub min_n:u32, pub max_n:u32, pub min_p:u32, pub max_p:u32,
}
fn bytes(r:&[u8],p:usize,n:usize)->Result<&[u8],&'static str> {
    r.get(p..p.checked_add(n).ok_or("PLL table overflow")?).ok_or("PLL table outside ROM")
}
fn word(r:&[u8],p:usize)->Result<usize,&'static str> {Ok(u16::from_le_bytes(bytes(r,p,2)?.try_into().unwrap()) as usize)}
pub fn limits(r:&[u8])->Result<Limits,&'static str> {
    let bit=r.windows(6).position(|s|s==b"\xff\xb8BIT\0").ok_or("BIT missing")?;
    let h=bytes(r,bit,12)?;
    if h[8]!=12 || h[9]!=6 || h[10]==0 || h.iter().fold(0u8,|s,b|s.wrapping_add(*b))!=0 {return Err("BIT header");}
    let mut table=None;
    for e in bytes(r,bit+12,h[10] as usize*6)?.chunks_exact(6).filter(|e|e[0]==b'C') {
        if table.is_some() {return Err("duplicate BIT C");}
        let data=bytes(r,word(e,4)?,word(e,2)?)?;
        table=Some(match e[1] {
            1 if data.len()>=10=>word(data,8)?,
            2 if data.len()>=4=>u32::from_le_bytes(data[..4].try_into().unwrap()) as usize,
            _=>return Err("BIT C version/size"),
        });
    }
    let table=table.filter(|t|*t!=0).ok_or("PLL table missing")?;
    let h=bytes(r,table,4)?;
    if h[0]!=0x40 || h[1]<4 || h[2]<11 || h[3]==0 {return Err("PLL table version/geometry");}
    let entries=bytes(r,table+h[1] as usize,h[2] as usize*h[3] as usize)?;
    let mut found=None;
    for e in entries.chunks_exact(h[2] as usize) {
        if u32::from_le_bytes(e[3..7].try_into().unwrap())!=0x137000 {continue;}
        if found.is_some() {return Err("duplicate GPC PLL");}
        let d=bytes(r,word(e,1)?,14)?;
        let l=Limits {min_vco:word(d,0)? as u32*1000,max_vco:word(d,2)? as u32*1000,
            min_input:word(d,4)? as u32*1000,max_input:word(d,6)? as u32*1000,
            min_m:d[8] as u32,max_m:d[9] as u32,min_n:d[10] as u32,max_n:d[11] as u32,
            min_p:d[12] as u32,max_p:d[13] as u32};
        if l.min_vco==0 || l.min_vco>l.max_vco || l.min_input==0 || l.min_input>l.max_input ||
            l.min_m==0 || l.min_m>l.max_m || l.min_n==0 || l.min_n>l.max_n ||
            l.min_p==0 || l.min_p>l.max_p || l.max_p>63 {return Err("GPC PLL limits invalid");}
        found=Some(l);
    }
    found.ok_or("GPC PLL missing")
}
#[derive(Debug)]
pub struct Pll {pub coefficient:u32,pub khz:u32}
pub fn pll(l:&Limits,reference:u32,requested:u32)->Result<Pll,&'static str> {
    if requested==0 || reference==0 {return Err("zero PLL clock");}
    let p=(l.max_vco/requested).clamp(l.min_p,l.max_p);
    let low=((reference+l.max_input)/l.max_input).max(l.min_m);
    let high=((reference+l.min_input)/l.min_input).min(l.max_m);
    let mut best=None;let mut error=u32::MAX;
    for m in low.min(high)..=high {
        let tmp=u64::from(requested)*u64::from(p)*u64::from(m);
        let mut n=(tmp/u64::from(reference)) as u32;
        if tmp%u64::from(reference)>=u64::from(reference/2) {n+=1;}
        if n<l.min_n {continue;}if n>l.max_n {break;}
        let khz=(u64::from(reference)*u64::from(n)/u64::from(m)/u64::from(p)) as u32;
        let e=requested.abs_diff(khz);
        if e<error {error=e;best=Some(Pll {coefficient:(p<<16)|(n<<8)|m,khz});}
    }
    best.ok_or("no matching GPC PLL coefficients")
}
#[derive(Debug)]
pub struct Plan {pub khz:u32,pub pll_selected:bool,pub source_divider:u32,pub coefficient:u32}
fn divide(reference:u32,requested:u32)->(u32,u32) {
    let d=(reference.saturating_mul(2)/requested).clamp(2,65);
    (reference*2/d,d-2)
}
pub fn plan(domain:&str,requested:u32,post:u32,l:&Limits,reference:u32,vco:u32)->Result<Plan,&'static str> {
    if domain!="gpc" {return Err("GPC-only");}
    if requested==0 || requested>post.min(810_000) {return Err("above-POST-or-zero");}
    if vco==0 {return Err("VCO unavailable");}
    let (first,div)=divide(vco,requested);let (divided,final_div)=divide(first,requested);
    // C2 keeps GPC's POST final divider at unity (gk104 prog stage 3 excludes idx 0).
    if final_div!=0 {return Err("GPC final divider unsupported");}
    let candidate=if divided!=requested {pll(l,reference,requested).ok()}else{None};
    if let Some(p)=candidate {
        if requested.abs_diff(p.khz)<requested.abs_diff(divided) {
            return Ok(Plan {khz:p.khz,pll_selected:true,source_divider:0,coefficient:p.coefficient});
        }
    }
    Ok(Plan {khz:divided,pll_selected:false,source_divider:div,coefficient:0})
}
pub trait Access {
    fn read(&mut self,address:u32)->u32;
    fn write(&mut self,address:u32,value:u32);
    fn wait(&mut self,address:u32,mask:u32,value:u32)->Result<(),&'static str>;
}
fn mask(io:&mut impl Access,address:u32,bits:u32,value:u32) {
    let old=io.read(address);io.write(address,(old&!bits)|value);
}
pub fn program(io:&mut impl Access,p:&Plan)->Result<(),&'static str> {
    // MIT gk104.c:356-473, restricted to GPC idx 0; no other domain is visited.
    if !p.pll_selected {
        let d=if p.source_divider!=0 {0x80000000|p.source_divider}else{0};
        mask(io,0x1371d0,0x8000003f,d);io.write(0x137160,3);
    }
    mask(io,0x137100,1,0);io.wait(0x137100,1,0)?;
    mask(io,0x137000,4,0);mask(io,0x137000,1,0);
    if p.pll_selected {
        io.write(0x137004,p.coefficient);mask(io,0x137000,1,1);
        mask(io,0x137000,0x10,0);io.wait(0x137000,0x20000,0x20000)?;
        mask(io,0x137000,0x10,0x10);mask(io,0x137000,4,4);
        mask(io,0x137100,1,1);io.wait(0x137100,1,1)?;
    }
    Ok(())
}

#[path = "c2/observe.rs"]
pub mod observe;

#[cfg(target_os="none")]
mod runtime;
#[cfg(target_os="none")]
pub use runtime::{read_status,descriptor};
#[cfg(target_os="none")]
pub(super) use runtime::initialize;
