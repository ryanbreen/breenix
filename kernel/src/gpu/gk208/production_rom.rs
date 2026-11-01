/*
 * Copyright 2012 Nouveau Community
 * Copyright 2011 Red Hat Inc.
 * Copyright 2012 Red Hat Inc.
 * Copyright 2013 Red Hat Inc.
 * Copyright 2014 Red Hat Inc.
 * Copyright 2014 Martin Peres
 * Copyright 2015 Martin Peres
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
 * Authors: Martin Peres
 */
//! Runtime BIT M/P policy. Layouts and arithmetic: MIT nouveau v6.8
//! bios/{M0203,perf,volt,vmap,gpio}.c, volt/{base,gpio}.c and gpio/gf119.c.
//! Unsupported layouts return POST fallback; voltage access is read-only.
use super::{modeset,clock::perf};
#[derive(Debug)]
pub struct Target {pub gpc_khz:u32,pub memory_khz:u32,pub voltage_id:u8}
fn n(r:&[u8],p:usize,s:usize)->Result<u32,&'static str> {
 let bytes=r.get(p..p.checked_add(s).ok_or("voltage table overflow")?).ok_or("voltage table bounds")?;
 Ok(bytes.iter().enumerate().fold(0,|v,(i,b)|v|u32::from(*b)<<(8*i)))
}
pub fn target(r:&[u8],strap:u32)->Result<Target,&'static str> {
 let (v,m,len)=modeset::bit(r,b'M').map_err(|_|"BIT M missing")?;
 if v!=2 || len<5 {return Err("RAM type table unavailable");}
 let t=n(r,m+3,2)? as usize;
 if t==0 || n(r,t,1)?!=0x10 || n(r,t+1,1)?<7 || n(r,t+2,1)?<2 || n(r,t+4,1)?!=0 {return Err("RAM type geometry");}
 let mut ty=None;
 for i in 0..n(r,t+3,1)? as usize {
  let q=t+n(r,t+1,1)? as usize+i*n(r,t+2,1)? as usize;
  let b=n(r,q,1)?;
  if b&15!=15 && b>>4==(strap&0x3c)>>2 {ty=Some(b&15);break;}
 }
 if ty!=Some(1) {return Err("production memory reclock requires DDR3");}
 let table=perf::decode(r)?;
 let level=table.levels.iter().max_by_key(|l|l.gpc_khz).ok_or("no performance level")?;
 if level.gpc_khz==0 || level.memory_khz==0 {return Err("zero performance clock");}
 Ok(Target {gpc_khz:level.gpc_khz,memory_khz:level.memory_khz,voltage_id:level.voltage_id})
}
fn requirement(r:&[u8],t:usize,id:u8,temp:u32,speedo:i32,seen:&mut [bool;256])->Result<u32,&'static str> {
 if t==0 || id as u32>=n(r,t+3,1)? {return if id==0 {Err("voltage map absent")}else{Ok(u32::from(id)*10000)};}
 if seen[id as usize] {return Err("cyclic voltage map");}seen[id as usize]=true;
 let v=n(r,t,1)?;let h=n(r,t+1,1)? as usize;let len=n(r,t+2,1)? as usize;
 if h<4 || !matches!(v,0x10|0x20) || len<if v==0x10 {20}else{34} {return Err("voltage map geometry");}
 let q=t+h+id as usize*len;let off=if v==0x10 {0}else{2};
 let lo=n(r,q+off,4)?;let hi=n(r,q+off+4,4)?;
 if lo>hi || speedo<0 {return Err("voltage map input invalid");}
 let mut a=[0i128;6];for (i,a) in a.iter_mut().enumerate().take(if v==0x10 {3}else{6}) {*a=n(r,q+off+8+i*4,4)? as i32 as i128;}
 let s=i128::from(speedo);let temp=i128::from(temp);let mode=if v==0x10 {0}else{n(r,q,1)?};
 let value=match mode {
  0=>a[0]/10+a[1]*s/10+a[2]*s*s/100000,
  1=>((a[0]*15625)>>18)+((a[1]*s*15625)>>18)+((a[2]*temp*15625)>>10)
    +((a[3]*s*temp*15625)>>18)+((a[4]*s*s*15625)>>30)+((a[5]*temp*temp*15625)>>18),
  3=>(i128::from(lo)+i128::from(hi))/2,
  _=>i128::from(lo),
 }.clamp(i128::from(lo),i128::from(hi)) as u32;
 let link=if v==0x10 {255}else{n(r,q+1,1)? as u8};
 let extra=if link==255 {0}else{requirement(r,t,link,temp as u32,speedo,seen)?};
 value.checked_add(extra).ok_or("voltage map overflow")
}
/// Return actual VID voltage and the rounded-up target level requirement.
pub fn voltage(r:&[u8],id:u8,temp:u32,speedo:i32,read:&mut impl FnMut(u32)->u32)->Result<(u32,u32),&'static str> {
 let (v,p,len)=modeset::bit(r,b'P').map_err(|_|"BIT P missing")?;
 if v!=2 || len<0x24 {return Err("BIT P voltage pointers unavailable");}
 let t=n(r,p+0x20,4)? as usize;
 let required=requirement(r,t,id,temp,speedo,&mut [false;256])?;
 let t=n(r,p+0x0c,4)? as usize;
 if t==0 {return Err("voltage table absent");}
 let v=n(r,t,1)?;let h=if v==0x12 {5}else{n(r,t+1,1)? as usize};
 let (count,size)=match v {0x12=>(n(r,t+2,1)?,n(r,t+1,1)?),0x20=>(n(r,t+2,1)?,n(r,t+3,1)?),_=>(n(r,t+3,1)?,n(r,t+2,1)?)};
 let mask=n(r,t+match v {0x12|0x30=>4,0x20=>5,0x40=>11,0x50=>6,_=>return Err("voltage version")},1)?;
 if v==0x50 && n(r,t+4,1)?&1!=0 {return Err("PWM voltage unsupported");}
 let dcb=n(r,0x36,2)? as usize;let gpio=n(r,dcb+10,2)? as usize;
 if gpio==0 || n(r,gpio,1)?!=0x41 || n(r,gpio+3,1)?<5 {return Err("VID GPIO geometry");}
 let mut vid=0u32;
 for (bit,tag) in [4,5,6,0x1a,0x73,0x74,0x75,0x76].into_iter().enumerate() {
  if mask&(1<<bit)==0 {continue;}
  for i in 0..n(r,gpio+2,1)? as usize {
   let q=gpio+n(r,gpio+1,1)? as usize+i*n(r,gpio+3,1)? as usize;let info=n(r,q,4)?;
   if (info>>8)&255!=tag {continue;}
   let raw=read(0xd610+(info&63)*4);
   if raw>>16==0xbadf || raw==u32::MAX {return Err("VID GPIO read failed");}
   let log1=(n(r,q+4,1)?>>6)&1;
   if (raw>>14)&1==log1 {vid|=1<<bit;}break;
  }
 }
 let ranged=v==0x40 || v==0x50 && n(r,t+4,1)?&2!=0;
 let mut current=None;let mut selected=None;
 let mut consider=|i:u32,uv:u32| {
  if i==vid {current=Some(uv);}
  if uv>=required {selected=Some(selected.map_or(uv,|old:u32|old.min(uv)));}
 };
 if ranged {
  let base=n(r,t+if v==0x40 {4}else{0x12},4)?&if v==0x40 {u32::MAX}else{0xffffff};
  let step=n(r,t+if v==0x40 {8}else{0x16},2)? as i16 as i64;
  let end=i64::from(base)+step*i64::from(mask);
  let lo=if v==0x40 {i64::from(base).min(end)}else{i64::from(n(r,t+0x0a,4)?)};
  let hi=n(r,t+0x0e,4)?;let hi=if v==0x40 && hi==0 {i64::from(base).max(end)}else{i64::from(hi)};
  for i in 0..=mask {let uv=i64::from(base)+step*i64::from(i);if uv>=lo && uv<=hi {consider(i,uv as u32);}}
 }else{
  for i in 0..count as usize {
   let q=t+h+i*size as usize;
   let (i,uv)=match v {
    0x12|0x20|0x30=>(n(r,q+1,1)?>>if v==0x30 {2}else{0},n(r,q,1)?*10000),
    0x50=>{let w=n(r,q,4)?;((w>>23)&255,w&0x1fffff)},
    _=>return Err("voltage entry layout"),
   };consider(i,uv);
  }
 }
 Ok((current.ok_or("current VID absent")?,selected.ok_or("no VID meets requirement")?))
}
