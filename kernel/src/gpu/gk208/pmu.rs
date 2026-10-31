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
//! C4: MIT nouveau v6.8 pmu/gk208.c:30-44 selects gf100 reset and
//! gt215.c:202-260 upload/start/ring readiness. MC gk104.c:27-30 and
//! nv04.c:34-57 supply PMU's PMC bit and masked reset. No PMU messages.
pub trait Access {
    fn read(&mut self,address:u32)->u32;
    fn write(&mut self,address:u32,value:u32);
    fn now_us(&mut self)->u64;
    fn waited(&mut self,name:&'static str,raw:u32,elapsed_us:u64,matched:bool);
}
#[derive(Debug)]
pub struct Rings {pub send:u32,pub recv:u32}
fn wait(io:&mut impl Access,name:&'static str,address:u32,ready:impl Fn(u32)->bool)->Result<u32,&'static str> {
    let begin=io.now_us();
    loop {
        let raw=io.read(address);
        let elapsed=io.now_us().saturating_sub(begin);
        let matched=ready(raw);
        if matched || elapsed>=2_000_000 {
            io.waited(name,raw,elapsed,matched);
            return if matched {Ok(raw)}else{Err("PMU ring descriptor timeout")};
        }
        core::hint::spin_loop();
    }
}
pub fn start(io:&mut impl Access,code:&[u8],data:&[u8])->Result<Rings,&'static str> {
    if code.is_empty() || data.is_empty() || code.len()%4!=0 || data.len()%4!=0 {return Err("invalid PMU image words");}
    if io.read(0x200)&0x2000!=0 {
        io.write(0x10a014,0xffff);
        // Nouveau observes these two bounded waits without returning failure.
        let _=wait(io,"idle",0x10a04c,|v|v==0);
    }
    let old=io.read(0x200);io.write(0x200,old&!0x2000);
    let old=io.read(0x200);io.write(0x200,old|0x2000);
    let _=io.read(0x200);
    let _=wait(io,"scrub",0x10a10c,|v|v&6==0);
    io.write(0x10a1c0,0x01000000);
    for word in data.chunks_exact(4) {io.write(0x10a1c4,u32::from_le_bytes(word.try_into().unwrap()));}
    io.write(0x10a180,0x01000000);
    for (i,word) in code.chunks_exact(4).enumerate() {
        if i&0x3f==0 {io.write(0x10a188,(i>>6) as u32);}
        io.write(0x10a184,u32::from_le_bytes(word.try_into().unwrap()));
    }
    io.write(0x10a10c,0);io.write(0x10a104,0);io.write(0x10a100,2);
    let send=wait(io,"send",0x10a4d0,|v|v!=0)?;
    let recv=wait(io,"recv",0x10a4dc,|v|v!=0)?;
    io.write(0x10a010,0xe0);
    Ok(Rings {send,recv})
}
