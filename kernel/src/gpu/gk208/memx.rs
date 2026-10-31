/*
 * Copyright 2012 Red Hat Inc.
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
 * claim-lint:ok: Original MIT grant, pinned source hashes in docs/gui/provenance/gt710-c5a-sources.json.
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
 * Copyright 2010 Red Hat Inc.
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
 * claim-lint:ok: Original MIT grant, pinned source hashes in docs/gui/provenance/gt710-c5a-sources.json.
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
 * Copyright 2014 Red Hat Inc.
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
 * claim-lint:ok: Original MIT grant, pinned source hashes in docs/gui/provenance/gt710-c5a-sources.json.
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
 * claim-lint:ok: Original MIT grant, pinned source hashes in docs/gui/provenance/gt710-c5a-sources.json.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
 * THE AUTHORS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
 * WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF
 * OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
 * SOFTWARE.
 */
// SPDX-License-Identifier: MIT
//! Probe-only live DDR3 calculation and PMU memx transport.
//! Adapted from checked MIT nouveau v6.8 bios/{rammap,timing,ramcfg,M0203,pll,gpio}.c,
//! clk/pllgt215.c, fb/{ramgk104,sddr3}.c, ramfuc.h, pmu/{memx,gt215}.c and fuc/os.h.
//! K5 bounds execution to 405000 -> 900000 -> 405000 kHz, in mode 1.
use alloc::{collections::BTreeMap, vec::Vec};
use super::modeset;
type Config = BTreeMap<&'static str,u32>;
type Field = (&'static str,usize,usize,u32,u32);
const RAMMAP: &[Field] = &[
    ("rammap_min",0,2,0xffff,0),
    ("rammap_max",2,2,0xffff,0),
    ("rammap_11_08_01",8,1,0x1,0),
    ("rammap_11_08_0c",8,1,0xc,2),
    ("rammap_11_08_10",8,1,0x10,4),
    ("rammap_11_09_01ff",9,4,0x1ff,0),
    ("rammap_11_0a_03fe",9,4,0x3fe00,9),
    ("rammap_11_0a_0400",9,4,0x40000,18),
    ("rammap_11_0a_0800",9,4,0x80000,19),
    ("rammap_11_0b_01f0",9,4,0x1f00000,20),
    ("rammap_11_0b_0200",9,4,0x2000000,25),
    ("rammap_11_0b_0400",9,4,0x4000000,26),
    ("rammap_11_0b_0800",9,4,0x8000000,27),
    ("rammap_11_0d",13,1,0xff,0),
    ("rammap_11_0e",14,1,0xff,0),
    ("rammap_11_0f",15,1,0xff,0),
    ("rammap_11_11_0c",17,1,0xc,2),
];
const RAMCFG: &[Field] = &[
    ("ramcfg_11_01_01",1,1,0x1,0),
    ("ramcfg_11_01_02",1,1,0x2,1),
    ("ramcfg_11_01_04",1,1,0x4,2),
    ("ramcfg_11_01_08",1,1,0x8,3),
    ("ramcfg_11_01_10",1,1,0x10,4),
    ("ramcfg_DLLoff",1,1,0x20,5),
    ("ramcfg_11_01_40",1,1,0x40,6),
    ("ramcfg_11_01_80",1,1,0x80,7),
    ("ramcfg_11_02_03",2,1,0x3,0),
    ("ramcfg_11_02_04",2,1,0x4,2),
    ("ramcfg_11_02_08",2,1,0x8,3),
    ("ramcfg_11_02_10",2,1,0x10,4),
    ("ramcfg_11_02_40",2,1,0x40,6),
    ("ramcfg_11_02_80",2,1,0x80,7),
    ("ramcfg_11_03_0f",3,1,0xf,0),
    ("ramcfg_11_03_30",3,1,0x30,4),
    ("ramcfg_11_03_c0",3,1,0xc0,6),
    ("ramcfg_11_03_f0",3,1,0xf0,4),
    ("ramcfg_11_04",4,1,0xff,0),
    ("ramcfg_11_06",6,1,0xff,0),
    ("ramcfg_11_07_02",7,1,0x2,1),
    ("ramcfg_11_07_04",7,1,0x4,2),
    ("ramcfg_11_07_08",7,1,0x8,3),
    ("ramcfg_11_07_10",7,1,0x10,4),
    ("ramcfg_11_07_40",7,1,0x40,6),
    ("ramcfg_11_07_80",7,1,0x80,7),
    ("ramcfg_11_08_01",8,1,0x1,0),
    ("ramcfg_11_08_02",8,1,0x2,1),
    ("ramcfg_11_08_04",8,1,0x4,2),
    ("ramcfg_11_08_08",8,1,0x8,3),
    ("ramcfg_11_08_10",8,1,0x10,4),
    ("ramcfg_11_08_20",8,1,0x20,5),
    ("ramcfg_11_09",9,1,0xff,0),
];
const TIMING: &[Field] = &[
    ("timing_20_2e_03",46,1,0x3,0),
    ("timing_20_2e_30",46,1,0x30,4),
    ("timing_20_2e_c0",46,1,0xc0,6),
    ("timing_20_2f_03",47,1,0x3,0),
    ("timing_20_2c_003f",44,2,0x3f,0),
    ("timing_20_2c_1fc0",44,2,0x1fc0,6),
    ("timing_20_30_07",48,1,0x7,0),
    ("timing_20_30_f8",48,1,0xf8,3),
    ("timing_20_31_0007",49,2,0x7,0),
    ("timing_20_31_0078",49,2,0x78,3),
    ("timing_20_31_0780",49,2,0x780,7),
    ("timing_20_31_0800",49,2,0x800,11),
    ("timing_20_31_7000",49,2,0x7000,12),
    ("timing_20_31_8000",49,2,0x8000,15),
];
const PROG: &[(u32,&[(&str,u32,u32)])] = &[
    (0x10f468, &[("rammap_11_0a_03fe",0x1ff000,12),("rammap_11_09_01ff",0x1ff,0)]),
    (0x10f420, &[("rammap_11_0a_0400",0x1,0)]),
    (0x10f430, &[("rammap_11_0a_0800",0x1,0)]),
    (0x10f400, &[("rammap_11_0b_01f0",0x1f,0)]),
    (0x10f410, &[("rammap_11_0b_0200",0x200,9)]),
    (0x10f440, &[("rammap_11_0d",0xff0000,16),("rammap_11_0f",0xff00,8)]),
    (0x10f444, &[("rammap_11_0e",0xff00,8),("rammap_11_0b_0800",0x80,7),("rammap_11_0b_0400",0x20,5)]),
];
fn word(r:&[u8],p:usize,n:usize)->Result<u32,&'static str> {
 let b=r.get(p..p.checked_add(n).ok_or("memx ROM overflow")?).ok_or("memx ROM bounds")?;
 Ok(b.iter().enumerate().fold(0,|v,(i,b)|v|u32::from(*b)<<(i*8)))
}
fn fields(r:&[u8],p:usize,spec:&[Field],c:&mut Config)->Result<(),&'static str> {
 for &(name,offset,size,mask,shift) in spec {c.insert(name,(word(r,p+offset,size)?&mask)>>shift);} Ok(())
}
struct Tables {configs:Vec<Config>,pll:Config,gpios:Vec<(u32,u32,[u32;2])>}
fn tables(r:&[u8],strap:u32)->Result<Tables,&'static str> {
 let (mv,m,ml)=modeset::bit(r,b'M').map_err(|_|"BIT M missing")?;
 let crystal=match strap&0x400040 {0=>13500,0x40=>14318,0x400000=>27000,_=>25000};
 let strap=((strap&0x3c)>>2) as usize;
 let xlat=match mv {1 if ml>=5=>word(r,m+3,2)?,2 if ml>=3=>word(r,m+1,2)?,_=>return Err("BIT M version")};
 let mut index=if xlat==0 {strap as u32}else{word(r,xlat as usize+strap,1)?};
 if mv==2 && ml>=7 {
  let t=word(r,m+3,2)? as usize;
  if t!=0 && word(r,t,1)?==0x10 && word(r,t+4,1)?==0 {
   for i in 0..word(r,t+3,1)? as usize {
    let e=t+word(r,t+1,1)? as usize+i*word(r,t+2,1)? as usize;
    if word(r,e,1)?>>4==strap as u32 {index=word(r,e+1,1)?&15;break;}
   }
  }
 }
 let (pv,p,pl)=modeset::bit(r,b'P').map_err(|_|"BIT P missing")?;
 if pv!=2 || pl<12 {return Err("BIT P geometry");}
 let map=word(r,p+4,4)? as usize;let timing=word(r,p+8,4)? as usize;
 let h=word(r,map+1,1)? as usize;let b=word(r,map+2,1)? as usize;
 let s=word(r,map+3,1)? as usize;let n=word(r,map+4,1)? as usize;
 if word(r,map,1)?!=0x11 || h<6 || b<0x12 || s<10 || index as usize>=n || word(r,timing,1)?!=0x20 {return Err("DDR3 table geometry");}
 let mut configs=Vec::new();
 for i in 0..word(r,map+5,1)? as usize {
  let q=map+h+i*(b+s*n);let cfg=q+b+index as usize*s;let mut c=Config::new();
  fields(r,q,RAMMAP,&mut c)?;fields(r,cfg,RAMCFG,&mut c)?;
  let ti=word(r,cfg,1)? as usize;
  if ti>=word(r,timing+5,1)? as usize {return Err("DDR3 timing index");}
  let q=timing+word(r,timing+1,1)? as usize+ti*(word(r,timing+2,1)? as usize+word(r,timing+3,1)? as usize*word(r,timing+4,1)? as usize);
  fields(r,q,TIMING,&mut c)?;
  for j in 0..11 {c.insert(TIMING_NAMES[j],word(r,q+j*4,4)?);}
  configs.push(c);
 }
 let (cv,c,cl)=modeset::bit(r,b'C').map_err(|_|"BIT C missing")?;
 let p=if cv==1 && cl>=10 {word(r,c+8,2)?}else{word(r,c,4)?} as usize;
 if word(r,p,1)?!=0x40 || word(r,p+2,1)?<11 {return Err("memx PLL geometry");}
 let mut pll=Config::new();
 for i in 0..word(r,p+3,1)? as usize {
  let q=p+word(r,p+1,1)? as usize+i*word(r,p+2,1)? as usize;
  if word(r,q,1)?!=0x0c {continue;}
  let d=word(r,q+1,2)? as usize;
  for (j,name) in ["min_freq","max_freq","min_inputfreq","max_inputfreq"].iter().enumerate() {pll.insert(name,word(r,d+j*2,2)?*1000);}
  for (j,name) in ["min_m","max_m","min_n","max_n","min_p","max_p"].iter().enumerate() {pll.insert(name,word(r,d+8+j,1)?);}
  let refclk=word(r,q+9,2)?*1000;
  pll.insert("refclk",if refclk!=0 {refclk}else{crystal});
 }
 if pll.is_empty() || pll["refclk"]==0 {return Err("memx reference PLL absent");}
 let dcb=word(r,0x36,2)? as usize;let gpio=word(r,dcb+10,2)? as usize;let mut gpios=Vec::new();
 if gpio!=0 {
  if word(r,gpio,1)?!=0x41 {return Err("memx GPIO version");}
  for i in 0..word(r,gpio+2,1)? as usize {
   let q=gpio+word(r,gpio+1,1)? as usize+i*word(r,gpio+3,1)? as usize;
   let info=word(r,q,4)?;let tag=(info>>8)&255;let logs=word(r,q+4,1)?;
   if tag==0x18 || tag==0x2e {gpios.push((tag,0xd610+(info&63)*4,[(((logs>>4)&3)^2)<<12,(((logs>>6)&3)^2)<<12]));}
  }
 }
 Ok(Tables {configs,pll,gpios})
}
const TIMING_NAMES:[&str;11]=["t0","t1","t2","t3","t4","t5","t6","t7","t8","t9","t10"];
fn select(t:&Tables,khz:u32)->Result<Config,&'static str> {
 t.configs.iter().find(|c|c["rammap_min"]<=khz/1000 && khz/1000<=c["rammap_max"]).cloned().ok_or("memx RAMMAP range")
}
fn pll(c:&Config,freq:u32)->Result<[u32;4],&'static str> {
 let reference=c["refclk"];let p=(c["max_freq"]/freq).clamp(c["min_p"],c["max_p"]);
 let lo=((reference+c["max_inputfreq"])/c["max_inputfreq"]).max(c["min_m"]);
 let hi=((reference+c["min_inputfreq"])/c["min_inputfreq"]).min(c["max_m"]);
 for m in lo.min(hi)..=hi {
  let tmp=u64::from(freq)*u64::from(p)*u64::from(m);let reference=u64::from(reference);
  let mut n=tmp/reference;if tmp%reference<reference/2 {n-=1;}
  if n<u64::from(c["min_n"]) {continue;}if n>u64::from(c["max_n"]) {break;}
  let frac=tmp-n*reference;let fn_=(((frac<<13)+reference/2)/reference).wrapping_sub(4096) as u16;
  return Ok([m,n as u32,p,fn_ as u32]);
 }
 Err("memx PLL calculation")
}
#[derive(Clone,Debug,PartialEq,Eq)]
pub struct Op {pub code:u32,pub args:Vec<u32>}
pub struct Plan {pub ops:Vec<Op>,pub cpu_before:Vec<(u32,u32)>,pub cpu_after:Vec<(u32,u32)>,pub coefficients:[u32;4]}
impl Plan {
 pub fn words(&self)->Vec<u32> {
  let mut out=Vec::new();let mut method=0;let mut cache=Vec::new();
  fn flush(out:&mut Vec<u32>,method:&mut u32,cache:&mut Vec<u32>) {
   if *method!=0 {out.push(((cache.len() as u32)<<16)|*method);out.append(cache);*method=0;}
  }
  for op in &self.ops {
   if cache.len()+op.args.len()>=64 || method!=0 && method!=op.code {flush(&mut out,&mut method,&mut cache);}
   method=op.code;cache.extend_from_slice(&op.args);
   if op.code==4 || op.code==5 {flush(&mut out,&mut method,&mut cache);}
  }
  flush(&mut out,&mut method,&mut cache);out
 }
}
struct Script<'a,F:FnMut(u32)->u32> {read:&'a mut F,state:BTreeMap<u32,u32>,ops:Vec<Op>}
impl<F:FnMut(u32)->u32> Script<'_,F> {
 fn r(&mut self,a:u32)->u32 {self.state.get(&a).copied().unwrap_or_else(||(self.read)(a))}
 fn cmd(&mut self,code:u32,args:&[u32]) {self.ops.push(Op {code,args:args.to_vec()});}
 fn w(&mut self,a:u32,v:u32) {self.state.insert(a,v);self.cmd(3,&[a,v]);}
 fn m(&mut self,a:u32,mask:u32,data:u32) {let old=self.r(a);let new=(old&!mask)|data;if old!=new {self.w(a,new);}}
 fn ns(&mut self,n:u32) {self.cmd(5,&[n]);}
 fn dll(&mut self) {let v=self.r(0x10f300);self.w(0x10f300,v|0x100);self.m(0x10f300,0x100,0);}
 fn gpio(&mut self,gpios:&[(u32,u32,[u32;2])],tag:u32,index:usize) {
  for &(t,a,v) in gpios {if t==tag {let old=self.r(a);self.m(a,0x3000,v[index]);if old!=self.r(a) {self.w(0xd604,1);self.ns(if tag==0x18 {64000}else{20000});}}}
 }
}
fn prog(t:&Tables,c:&Config,read:&mut impl FnMut(u32)->u32)->Vec<(u32,u32)> {
 PROG.iter().map(|&(a,fs)| {
  let mut mask=0;let mut data=0;
  for &(name,bits,shift) in fs {if t.configs.iter().any(|d|d[name]!=t.configs[0][name]) {mask|=bits;data|=c[name]<<shift;}}
  (a,(read(a)&!mask)|data)
 }).collect()
}
/// K4 authorizes only the first xition of a future 900000 kHz request.
/// Current register values are read through the caller at calculation time.
pub fn calculate(r:&[u8],strap:u32,current:u32,read:&mut impl FnMut(u32)->u32)->Result<Plan,&'static str> {
 if current.abs_diff(405000)>4050 {return Err("outside C5d intermediate envelope");}
 calculate_pass(r,strap,405000,900000,true,read)
}
/// One nouveau calc_xits pass, reading hardware anew after the prior EXEC.
/// The descending transition uses target config plus the former's three
/// transition fields, then calculates the final target in a separate pass.
pub fn calculate_pass(r:&[u8],strap:u32,former:u32,target:u32,intermediate:bool,read:&mut impl FnMut(u32)->u32)->Result<Plan,&'static str> {
 if !matches!((former,target),(405000,900000)|(900000,405000)) {return Err("outside C5e memory envelope");}
 let t=tables(r,strap)?;
 let freq=if intermediate {former.min(target)}else{target};
 let mut c=select(&t,freq)?;
 if intermediate {
  let other=select(&t,former.max(target))?;
  for name in ["ramcfg_11_02_04","ramcfg_11_02_03","timing_20_30_07"] {c.insert(name,other[name]);}
 }
 let coefficients=pll(&t.pll,freq)?;
 let [m,n,p,fn_]=coefficients;let rcoef=(p<<16)|(n<<8)|m;
 let mut s=Script {read,state:BTreeMap::new(),ops:Vec::new()};
 let cl=c["t1"]&31;let cwl=(c["t1"]&0xf80)>>7;let wr=(c["t2"]&0x7f0000)>>16;
 let cl=match cl {5..=11=>(cl-4)*2,12..=14=>(cl-12)*2+1,_=>return Err("DDR3 CL")};
 let cwl=match cwl {5..=10=>cwl-5,_=>return Err("DDR3 CWL")};
 let wr=match wr {5..=8=>wr-4,10=>5,12=>6,14|15=>7,16=>0,_=>return Err("DDR3 WR")};
 let mr=[(s.r(0x10f300)&!0xf74)|((wr&7)<<9)|((cl&14)<<3)|((cl&1)<<2),
         (s.r(0x10f304)&!1)|c["ramcfg_DLLoff"],(s.r(0x10f320)&!0x38)|((cwl&7)<<3)];
 s.m(0x10f808,0x40000000,0x40000000);s.cmd(1,&[]);s.w(0x62c000,0x0f0f0000);
 let vc=c["ramcfg_11_02_08"]==0;let mv=c["ramcfg_11_02_04"]==0;
 if vc {s.gpio(&t.gpios,0x2e,1);}
 s.m(0x10f200,0x800,0);
 if c["ramcfg_11_03_f0"]!=0 {s.m(0x10f808,0x4000000,0x4000000);}
 s.w(0x10f314,1);
 if c["ramcfg_DLLoff"]!=0 && s.r(0x10f304)&1!=1 {s.m(0x10f304,1,1);s.ns(1000);}
 s.w(0x10f210,0);s.w(0x10f310,1);s.m(0x10f200,0x80000000,0x80000000);
 s.w(0x10f310,1);s.m(0x10f200,0x80000000,0);s.ns(1000);
 s.w(0x10f090,0x60);s.w(0x10f090,0xc000007e);
 let mut mask=0x800907e0;let mut data=mask;
 data&=![0x400,0x80000000,0x100,0x40][c["ramcfg_11_03_c0"] as usize];
 data&=![0x200,0x80000,0x80,0x20][c["ramcfg_11_03_30"] as usize];
 for (name,bits) in [("ramcfg_11_02_80",0x3000000),("ramcfg_11_02_40",0x2000),("ramcfg_11_07_10",0x4000)] {if c[name]!=0 {mask|=bits;}}
 mask|=if c["ramcfg_11_07_08"]!=0 {3}else{0x14000000};
 s.m(0x10f824,mask,data);s.m(0x132040,0x10000,0);
 s.m(0x1373f4,0,0x10010);
 let v=(s.r(0x1373ec)&!0x30000)|(c["ramcfg_11_03_30"]<<16);s.w(0x1373ec,v);
 s.m(0x1373f4,3,0);s.m(0x1373f4,0x10,0);
 if s.r(0x132024)!=rcoef || s.r(0x132034)&0xffff!=fn_ {
  s.m(0x132000,1,0);s.m(0x132020,1,0);s.w(0x137320,0);
  s.m(0x132030,0xffff0000,fn_<<16);s.m(0x132034,0xffff,fn_);
  s.w(0x132024,rcoef);s.m(0x132028,0x80000,0x80000);s.m(0x132020,1,1);
  s.cmd(4,&[0x137390,0x20000,0x20000,64000]);s.m(0x132028,0x80000,0);
 }
 s.m(0x1373f4,0x10,0x10);s.m(0x1373f4,3,1);s.m(0x1373f4,0x10000,0);
 s.gpio(&t.gpios,0x18,usize::from(mv));
 if c["ramcfg_11_02_40"]!=0 || c["ramcfg_11_07_10"]!=0 {s.m(0x132040,0x10000,0x10000);s.ns(20000);}
 if c["ramcfg_11_07_40"]!=0 {s.m(0x10f670,0x80000000,0x80000000);}
 s.w(0x10f65c,0x11*c["rammap_11_11_0c"]);s.w(0x10f6b8,0x1010101*c["ramcfg_11_09"]);s.w(0x10f6bc,0x1010101*c["ramcfg_11_09"]);
 data=0;
 for (name,bits) in [("ramcfg_11_02_80",0x3000000),("ramcfg_11_02_40",0x2000),("ramcfg_11_07_10",0x4000)] {if c[name]==0 {data|=bits;}}
 data|=if c["ramcfg_11_07_08"]!=0 {0x14000000}else{3};
 s.m(0x10f824,0x10000,data);s.ns(1000);
 s.m(0x10f82c,0x100000,if c["ramcfg_11_08_01"]!=0 {0x100000}else{0});
 for (i,a) in [0x10f248,0x10f290,0x10f294,0x10f298,0x10f29c,0x10f2a0,0x10f2a4,0x10f2a8,0x10f2ac,0x10f2cc,0x10f2e8].into_iter().enumerate() {s.m(a,u32::MAX,c[TIMING_NAMES[(i+10)%11]]);}
 mask=0x33f00000;data=0;
 if c["ramcfg_11_01_04"]==0 {data|=0x20200000;}
 if c["ramcfg_11_07_80"]==0 {data|=0x12800000;}
 if c["ramcfg_11_03_f0"]!=0 {
  if c["rammap_11_08_0c"]!=0 {if c["ramcfg_11_07_80"]==0 {mask|=0x20;}else{data|=0x20;}mask|=0x08000004;}
  data|=0x04000000;
 }else{mask|=0x44000020;data|=0x08000004;}
 s.m(0x10f808,mask,data);s.w(0x10f870,0x11111111*c["ramcfg_11_03_0f"]);
 s.m(0x10f250,0x3f0,c["timing_20_2c_003f"]<<4);
 data=((c["t10"]&0x7f000000)>>24).max(c["timing_20_2c_1fc0"]);
 s.m(0x10f24c,0x7f000000,data<<24);s.m(0x10f224,0x1f0000,c["timing_20_30_f8"]<<16);
 s.w(0x10f090,0x4000007f);s.ns(1000);s.w(0x10f314,1);s.w(0x10f310,1);s.w(0x10f210,0x80000000);s.ns(1000);
 if c["ramcfg_DLLoff"]==0 {s.m(0x10f304,1,0);s.dll();}
 s.m(0x10f320,0xfff,mr[2]);s.m(0x10f304,u32::MAX,mr[1]);s.w(0x10f300,mr[0]);s.ns(1000);
 if c["ramcfg_DLLoff"]==0 {s.dll();s.ns(1000);}
 if !vc {s.gpio(&t.gpios,0x2e,0);}
 s.m(0x10f830,0x1000000,0x1000000);s.m(0x10f830,0x1000000,0);
 s.m(0x10f200,0x80000000,0x80000000);s.w(0x10f318,1);s.m(0x10f200,0x80000000,0);s.ns(1000);
 s.cmd(2,&[]);s.w(0x62c000,0x0f0f0f00);s.m(0x10f200,0x800,c["rammap_11_08_01"]<<11);
 let ops=s.ops;
 let cpu_before=prog(&t,&select(&t,1000)?,read);let cpu_after=prog(&t,&select(&t,freq)?,read);
 Ok(Plan {ops,cpu_before,cpu_after,coefficients})
}
/// Single synchronous caller, serialized by the existing GPU owner. Polling
/// replaces nouveau's recv worker, using the same ring indices/ownership.
pub trait Access {
 fn read(&mut self,address:u32)->u32;
 fn write(&mut self,address:u32,value:u32);
 fn now_us(&mut self)->u64;
}
fn acquire(io:&mut impl Access,owner:u32)->Result<(),&'static str> {
 let start=io.now_us();
 loop {
  io.write(0x10a580,owner);if io.read(0x10a580)==owner {return Ok(());}
  if io.now_us().saturating_sub(start)>=2_000_000 {return Err("PMU DMEM ownership timeout");}
 }
}
fn send(io:&mut impl Access,message:u32,data:[u32;2])->Result<[u32;2],&'static str> {
 let send=io.read(0x10a4d0)&65535;let recv=io.read(0x10a4dc)&65535;
 let addr=io.read(0x10a4a0);let start=io.now_us();
 while io.read(0x10a4b0)==(addr^8) {if io.now_us().saturating_sub(start)>=2_000_000 {return Err("PMU send FIFO full");}}
 acquire(io,1)?;
 io.write(0x10a1c0,0x01000000+send+((addr&7)<<4));
 for v in [0x584d454d,message,data[0],data[1]] {io.write(0x10a1c4,v);}
 io.write(0x10a4a0,(addr+1)&15);io.write(0x10a580,0);
 let start=io.now_us();
 loop {
  let disp=io.read(0x10a01c);let intr=io.read(0x10a008)&disp&!(disp>>16);
  if intr&!0xc0!=0 {return Err("PMU fault awaiting reply");}
  let get=io.read(0x10a4cc);
  if get!=io.read(0x10a4c8) {
   acquire(io,2)?;io.write(0x10a1c0,0x02000000+recv+((get&7)<<4));
   let packet=[io.read(0x10a1c4),io.read(0x10a1c4),io.read(0x10a1c4),io.read(0x10a1c4)];
   io.write(0x10a4cc,(get+1)&15);io.write(0x10a580,0);io.write(0x10a004,0x40);
   if packet[0]==0x584d454d && packet[1]==message {return Ok([packet[2],packet[3]]);}
  }
  if intr&0x80!=0 {io.write(0x10a004,0x80);}
  if io.now_us().saturating_sub(start)>=2_000_000 {return Err("PMU reply timeout");}
  core::hint::spin_loop();
 }
}
pub struct Session {pub base:u32,pub size:u32}
impl Session {
 pub fn init(io:&mut impl Access)->Result<Self,&'static str> {
  let [base,size]=send(io,0,[0,0])?;
  acquire(io,3)?;io.write(0x10a1c0,0x01000000|base);Ok(Self {base,size})
 }
 pub fn load(&self,io:&mut impl Access,words:&[u32])->Result<(),&'static str> {
  // A storage bound, not an additional hardware-state success predicate.
  if words.len()*4>self.size as usize {io.write(0x10a580,0);return Err("memx script exceeds PMU buffer");}
  for &v in words {io.write(0x10a1c4,v);}Ok(())
 }
 pub fn exec(self,io:&mut impl Access)->Result<[u32;2],&'static str> {
  let finish=io.read(0x10a1c0)&0xffffff;io.write(0x10a580,0);
  send(io,1,[self.base,finish])
 }
}
