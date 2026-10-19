//! Breenix's integer-only drawing API over syscall 416.
#![no_std]
extern crate alloc;
use alloc::vec::Vec;
use gpu::abi::{draw, stream::encode};
pub use gpu::abi::{draw::Instance, pipeline};
use libbreenix::gpu;

pub struct Context {
    handle: u64,
    words: Vec<u32>,
    clips: Vec<[i32; 4]>,
    bounds: [i32; 4],
}
#[derive(Clone, Copy)]
pub struct Fence {
    context: u64,
    handle: u64,
}
impl Fence {
    pub fn wait(self, timeout_ns: u64) -> Result<(), i64> {
        let (r, _) = gpu::wait(self.context, self.handle, timeout_ns);
        if r == 0 {
            Ok(())
        } else {
            Err(r)
        }
    }
    pub fn handle(self) -> u64 {
        self.handle
    }
}
impl Context {
    pub fn new() -> Result<Self, i64> {
        Ok(Self {
            handle: gpu::context_create()?,
            words: Vec::new(),
            clips: Vec::new(),
            bounds: [0; 4],
        })
    }
    pub fn handle(&self) -> u64 {
        self.handle
    }
    pub fn begin(&mut self, target: u64, width: u32, height: u32) {
        encode::begin(&mut self.words, target);
        self.bounds = [0, 0, width as i32, height as i32];
        self.clips.clear();
    }
    pub fn clear(&mut self, colour: u32) {
        encode::clear(&mut self.words, colour);
    }
    pub fn solid(&mut self, colour: u32, rectangles: &[[i32; 4]]) {
        encode::draw(&mut self.words, pipeline::SOLID, colour, rectangles);
    }
    pub fn copy(&mut self, texture: u64, instances: &[[i32; 6]]) {
        encode::draw_sampled(&mut self.words, pipeline::COPY, texture, instances);
    }
    pub fn set_clip(&mut self, rect: [i32; 4]) {
        self.clips.clear();
        self.clips.push(intersect(self.bounds, rect));
        self.scissor();
    }
    pub fn push_clip(&mut self, rect: [i32; 4]) {
        self.clips.push(intersect(
            self.clips.last().copied().unwrap_or(self.bounds),
            rect,
        ));
        self.scissor();
    }
    pub fn pop_clip(&mut self) {
        self.clips.pop();
        self.scissor();
    }
    fn scissor(&mut self) {
        let r = self.clips.last().copied().unwrap_or(self.bounds);
        self.words.extend([8, 6]);
        self.words.extend(r.map(|v| v as u32));
    }
    /// Preserve primitive order. The first overlap ends the current draw; the next
    /// command therefore receives a destination snapshot after that draw completes.
    pub fn draw(
        &mut self,
        pipeline: u32,
        colour: u32,
        texture: u64,
        buffer: u64,
        data_bytes: u32,
        instances: &[Instance],
    ) {
        if !draw::blended(pipeline) {
            draw::encode(
                &mut self.words,
                pipeline,
                colour,
                texture,
                buffer,
                data_bytes,
                instances,
            );
            return;
        }
        let clip = self.clips.last().copied().unwrap_or(self.bounds);
        let rects: Vec<_> = instances.iter().enumerate().filter_map(|(n,i)| {
            let r = intersect(intersect(i.rect, clip), self.bounds);
            (r[0]<r[2] && r[1]<r[3]).then_some((n,r.map(|v|v as u32)))
        }).collect();
        let mut first = 0;
        let mut start = 0;
        while start < rects.len() {
            let disjoint = |end| {
                let rs: Vec<_> = rects[start..end].iter().map(|&(_,r)|r).collect();
                draw::disjoint(&rs).expect("BGL split scratch allocation")
            };
            // Exponentially grow a disjoint prefix, then locate its first overlap.
            // Prefix disjointness is monotone, so this avoids pairwise comparisons.
            let mut good = start+1;
            let mut end = (start+2).min(rects.len());
            while end > good && disjoint(end) {
                good=end;
                end=(start+2*(end-start)).min(rects.len());
            }
            if good == rects.len() { break; }
            let mut bad=end;
            while good+1 < bad {
                let mid=good+(bad-good)/2;
                if disjoint(mid) { good=mid; } else { bad=mid; }
            }
            let split=rects[good].0;
            draw::encode(&mut self.words,pipeline,colour,texture,buffer,data_bytes,&instances[first..split]);
            first=split;
            start=good;
        }
        if first < instances.len() {
            draw::encode(
                &mut self.words,
                pipeline,
                colour,
                texture,
                buffer,
                data_bytes,
                &instances[first..],
            );
        }
    }
    pub fn submit(&mut self) -> Result<Fence, i64> {
        let (r, a) = gpu::submit(self.handle, &self.words);
        if r != 0 {
            return Err(r);
        }
        self.words.clear();
        Ok(Fence {
            context: self.handle,
            handle: a.fence,
        })
    }
    /// Present a scanout target in this context's fence sequence.
    pub fn present(&self, target: u64) -> Result<Fence, i64> {
        let (r, a) = gpu::present(self.handle, target);
        if r != 0 { return Err(r); }
        Ok(Fence { context: self.handle, handle: a.fence })
    }
    pub fn encoded(&self) -> &[u32] {
        &self.words
    }
}
impl Drop for Context {
    fn drop(&mut self) {
        let _ = gpu::context_destroy(self.handle);
    }
}
/// Display modes and their physical pixel scale.
pub fn modes() -> Result<gpu::abi::Modes, i64> {
    let (r, modes) = gpu::modes();
    if r == 0 { Ok(modes) } else { Err(r) }
}
/// Select the current display owner's mode.
pub fn set_mode(index: u32) -> Result<(), i64> {
    match gpu::set_mode(index) { 0 => Ok(()), e => Err(e) }
}
fn intersect(a: [i32; 4], b: [i32; 4]) -> [i32; 4] {
    let x = a[0].max(b[0]);
    let y = a[1].max(b[1]);
    [x, y, a[2].min(b[2]).max(x), a[3].min(b[3]).max(y)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::ManuallyDrop;
    #[test]
    fn split_preserves_overlap_order_and_half_open_edges() {
        let mut c=ManuallyDrop::new(Context { handle:0,words:Vec::new(),clips:Vec::new(),bounds:[0,0,16,16] });
        let instances:Vec<_>=[[0,0,4,4],[4,0,8,4],[3,0,5,4],[8,0,10,4]].into_iter()
            .map(|rect|Instance {rect,params:[0;16]}).collect();
        c.draw(pipeline::OVER,0,1,0,0,&instances);
        let len=c.words[1] as usize;
        assert_eq!(c.words[9],2);
        assert_eq!(c.words[len+9],2);
        assert_eq!(c.words.len(),len+c.words[len+1] as usize);
        assert_eq!(&c.words[draw::HEADER..draw::HEADER+4],&[0,0,4,4]);
        assert_eq!(&c.words[len+draw::HEADER..len+draw::HEADER+4],&[3,0,5,4]);
    }
    #[test]
    fn text_sized_disjoint_batch_remains_one_draw() {
        let mut c=ManuallyDrop::new(Context {handle:0,words:Vec::new(),clips:Vec::new(),bounds:[0,0,16384,4]});
        let instances:Vec<_>=(0..65536).map(|n|Instance {rect:[n%16384,n/16384,n%16384+1,n/16384+1],params:[0;16]}).collect();
        c.draw(pipeline::OVER,0,1,0,0,&instances);
        assert_eq!(c.words[9],65536);
        assert_eq!(c.words.len(),c.words[1] as usize);
    }
}
