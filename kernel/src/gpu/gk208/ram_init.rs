/*
 * Copyright 2012 Red Hat Inc.
 * Copyright 2013 Red Hat Inc.
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
//! C5c: nouveau Linux v6.8 fb/ramgk104.c:1397-1439, bios/ramcfg.c,
//! bios/M0203.c and bios/rammap.c (MIT); hashes in gt710-c5c-sources.json.
//! DDR3 has no training writes: ramgk104.c:1383-1389 takes the default case.
use super::modeset::{self,Io,Op,ScriptError};
use alloc::vec::Vec;
fn byte(r:&[u8],p:usize)->Result<u8,&'static str> {r.get(p).copied().ok_or("RAM init ROM bounds")}
fn word(r:&[u8],p:usize,n:usize)->Result<u32,&'static str> {
    let mut value=0;for i in 0..n {value|=u32::from(byte(r,p+i)?)<<(i*8);}Ok(value)
}
/// Parse before any RAM writes, using the strap cached for this decode.
pub fn scripts(r:&[u8],strap:u32)->Result<Vec<Vec<Op>>,ScriptError> {
    let (v,m,len)=modeset::bit(r,b'M')?;
    let (groups,mut xlat)=match v {
        1 if len>=5=>(byte(r,m+2)?,word(r,m+3,2)? as usize),
        2 if len>=3=>(byte(r,m)?,word(r,m+1,2)? as usize),
        _=>return Err("unsupported RAM restriction table".into()),
    };
    let strap=((strap&0x3c)>>2) as u8;
    let mut index=None;
    if v==2 && len>=7 {
        let t=word(r,m+3,2)? as usize;
        if t!=0 && byte(r,t)?==0x10 && byte(r,t+4)?==0 {
            let h=byte(r,t+1)? as usize;let l=byte(r,t+2)? as usize;
            for i in 0..byte(r,t+3)? as usize {
                let e=t+h+i*l;
                if byte(r,e)?>>4==strap {index=Some(byte(r,e+1)?&15);break;}
            }
        }
    }
    let index=match index {Some(i)=>i,None=> {
        xlat+=usize::from(strap);
        if xlat!=usize::from(strap) {byte(r,xlat)?}else{strap}
    }};
    let (v,p,_)=modeset::bit(r,b'P')?;
    if v!=2 {return Err("RAMMAP needs BIT P v2".into());}
    let p=word(r,p+4,4)? as usize;
    if p==0 || !matches!(byte(r,p)?,0x10|0x11) || byte(r,p+1)?<0x15 {
        return Err("RAMMAP init header unavailable".into());
    }
    let table=word(r,p+0x10,4)? as usize;let count=byte(r,p+0x14)? as usize;
    let mut scripts=Vec::new();
    for i in 0..count {
        scripts.push(modeset::ram_script(r,word(r,table+i*4,4)? as usize,groups,index)?);
    }
    Ok(scripts)
}
/// Source order and masks, with no write-echo or lock-bit success predicate.
pub fn execute(io:&mut impl Io,scripts:&[Vec<Op>]) {
    fn mask(io:&mut impl Io,a:usize,m:u32,v:u32) {let old=io.read(a);io.write(a,(old&!m)|v);}
    // MIT ramgk104.c:1424-1433. Save only selector bits, skip the current mode.
    let save=io.read(0x10f65c)&0xf0;
    for (i,script) in scripts.iter().enumerate() {
        if i!= (save>>4) as usize {
            mask(io,0x10f65c,0xf0,(i as u32)<<4);
            modeset::execute_ops(io,script);
        }
    }
    mask(io,0x10f65c,0xf0,save);
    // MIT ramgk104.c:1435-1437. DDR3 training returns success without writes.
    mask(io,0x10f584,0x11000000,0);
    io.write(0x10ecc0,0xffffffff);
    mask(io,0x10f160,0x10,0x10);
}
