//! Concurrent channel probes using the parent's checked hardware mechanisms.
use super::*;
pub(super) fn probe(regs:&mut Registers,layout:&mut memory::Layout,total:u64,e:&Engine,bar:usize,
    bi:u64,bd:u64,pte:u64,a:&mut Channel,b:&mut Channel,seed:u32)->Result<String,&'static str> {
    let baseline=layout.allocator.stats();
    activate_pair(regs,total,e,bi,bd,pte,a,b)?;
    let first=[a.put+1,b.put+1];let mut digests=[Vec::new(),Vec::new()];let mut blocked=[0u32;2];
    for (i,c) in [&mut *a,&mut *b].into_iter().enumerate() {
        let initial:Vec<_>=(0..SIZE).map(|n|pattern(n,if i==0 {seed}else{!seed})).collect();
        fill(regs,total,c.source.range.start,&initial)?;
        modeset::bar_flush(regs)?;
        c.execute(regs,total,e,bar,rect(SOURCE,DEST,8192,1,8192,8192))?;
    }
    // Each round has two independent complete transfers, under P then ~P.
    // Four disjoint chunks occupy the four immutable command/release slots.
    for _ in 0..65 {
        for poison in [0x55,0xaa] {
            let mut pending=[Vec::new(),Vec::new()];
            for (i,c) in [&mut *a,&mut *b].into_iter().enumerate() {
                system_fill(c,poison);
                for chunk in 0..4 {
                    let ticket=c.context.hold(Consumer::Queued)?;
                    let seq=c.submit(regs,total,bar,rect(DEST+chunk*2048,SYSTEM+chunk*2048,2048,1,2048,2048),true)?;
                    pending[i].push((ticket,c.request(seq,c.dest.range.start,c.generation)));
                }
                let before=c.put;
                if c.submit(regs,total,bar,rect(DEST,SYSTEM,1,1,1,1),true).is_ok() || c.put!=before {
                    return Err("full CE queue overwrote in-flight storage");
                }
                blocked[i]+=1;
            }
            // Both channels have four requests published before any wait/reuse.
            for (i,c) in [&mut *a,&mut *b].into_iter().enumerate() {
                for (ticket,request) in pending[i].drain(..) {
                    c.completed(regs,total,e,request,2_000_000)?.0.ok_or("concurrent CE release missing")?;
                    c.queue.complete(request.sequence)?;
                    unsafe {c.context.finish_device(ticket)?;}
                }
                let bytes=system_bytes(c);
                if bytes.iter().enumerate().any(|(n,v)|*v!=pattern(n,if i==0 {seed}else{!seed})) {
                    return Err("concurrent opposite full readback mismatch");
                }
                digests[i].push(format!("\"{}\"",crate::gk208::hash(&bytes)));
            }
        }
    }
    let mut channels=Vec::new();
    for (i,c) in [&mut *a,&mut *b].into_iter().enumerate() {
        channels.push(format!("{{\"id\":{},\"instance\":{},\"commands\":{},\"source\":{},\"dest\":{},\"epoch\":{},\"first\":{},\"last\":{},\"backpressure\":{},\"compares\":{},\"poisons\":[85,170],\"digests\":[{}]}}",
            c.id,c.context.space().instance.range.start,c.commands.range.start,c.source.range.start,c.dest.range.start,c.epoch,first[i],c.put,blocked[i],digests[i].len(),digests[i].join(",")));
    }
    a.drain(regs,e)?;b.drain(regs,e)?;
    words(regs,total,pte,&[0,0])?;modeset::activate_bar1(regs,bi,bd)?;
    if layout.allocator.stats()!=baseline {return Err("concurrent charges changed");}
    Ok(format!("\"schema\":1,\"seed\":{},\"channels\":[{}]",seed,channels.join(",")))
}
