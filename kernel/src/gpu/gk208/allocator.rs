//! Breenix-owned guarded VRAM allocator. BAR views have no allocation or charge.
//! Only explicitly released CPU-only allocations can be reused here; queued-work
//! ownership and retirement must be attached before engine clients are exposed.
use super::Range;
use alloc::vec::Vec;

const PAGE: u64 = 4096;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { Display, Engine, Probe }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Allocation {
    id: u64,
    pub range: Range,
    kind: Kind,
}
impl Allocation {
    pub fn guarded(self) -> Range {
        Range { start: self.range.start-PAGE, size: self.range.size+2*PAGE }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stats {
    pub reserved:u64, pub payload:u64, pub guards:u64, pub charged:u64,
    pub display:u64, pub engine:u64, pub probe:u64, pub free:u64, pub allocations:usize,
}
#[derive(Debug)]
pub struct Allocator {
    total:u64,
    reserved:Vec<Range>,
    live:Vec<Allocation>,
    next:u64,
}
impl Allocator {
    pub fn new(total:u64, reserved:&[Range]) -> Result<Self,&'static str> {
        if total==0 || total>1<<32 || total%PAGE!=0 {return Err("invalid VRAM extent");}
        let mut merged:Vec<Range>=Vec::new();
        let mut ranges=reserved.to_vec();
        for r in &mut ranges {
            let end=r.end().ok_or("reservation overflow")?;
            if r.size==0 || end>total {return Err("reservation outside VRAM");}
            r.start &= !(PAGE-1);
            r.size=((end+PAGE-1)&!(PAGE-1))-r.start;
        }
        ranges.sort_by_key(|r|r.start);
        for r in ranges {
            if let Some(last)=merged.last_mut() {
                if r.start<=last.end().ok_or("reservation overflow")? {
                    last.size=last.end().unwrap().max(r.end().unwrap())-last.start;
                    continue;
                }
            }
            merged.push(r);
        }
        Ok(Self{total,reserved:merged,live:Vec::new(),next:1})
    }
    pub fn reserved(&self)->&[Range] { &self.reserved }
    pub fn stats(&self)->Stats {
        let mut s=Stats{reserved:self.reserved.iter().map(|r|r.size).sum(),payload:0,guards:self.live.len() as u64*8192,
            charged:0,display:0,engine:0,probe:0,free:0,allocations:self.live.len()};
        for a in &self.live {
            s.payload+=a.range.size;
            match a.kind {Kind::Display=>s.display+=a.range.size,Kind::Engine=>s.engine+=a.range.size,Kind::Probe=>s.probe+=a.range.size}
        }
        s.charged=s.payload+s.guards;s.free=self.total-s.reserved-s.charged;s
    }
    pub fn allocate(&mut self,bytes:u64,align:u64,kind:Kind)->Result<Allocation,&'static str> {
        self.allocate_after(bytes,align,kind,PAGE)
    }
    pub(super) fn allocate_after(&mut self,bytes:u64,align:u64,kind:Kind,mut cursor:u64)->Result<Allocation,&'static str> {
        if bytes==0 || align<PAGE || !align.is_power_of_two() {return Err("invalid VRAM allocation");}
        let size=bytes.checked_add(PAGE-1).ok_or("allocation overflow")?&!(PAGE-1);
        if size>self.total || self.next==u64::MAX {return Err("VRAM allocation exhausted");}
        loop {
            let start=cursor.max(PAGE).checked_add(align-1).ok_or("alignment overflow")?&!(align-1);
            let end=start.checked_add(size).and_then(|v|v.checked_add(PAGE)).ok_or("allocation overflow")?;
            if end>self.total {return Err("VRAM allocation exhausted");}
            let candidate=Range{start:start-PAGE,size:size+2*PAGE};
            let bump=self.reserved.iter().copied().chain(self.live.iter().map(|a|a.guarded()))
                .filter(|r|r.overlaps(candidate)).filter_map(Range::end).max();
            if let Some(end)=bump {cursor=end+PAGE;continue;}
            self.live.try_reserve(1).map_err(|_|"VRAM metadata allocation failed")?;
            let a=Allocation{id:self.next,range:Range{start,size},kind};
            self.next+=1;self.live.push(a);return Ok(a);
        }
    }
    /// Caller must own all access: this does not imply GPU work has drained.
    pub fn release(&mut self,a:Allocation)->Result<(),&'static str> {
        let i=self.live.iter().position(|live|*live==a).ok_or("stale VRAM allocation")?;
        self.live.remove(i);Ok(())
    }
}
