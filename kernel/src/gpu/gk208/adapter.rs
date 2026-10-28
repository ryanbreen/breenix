//! Native requests under the shared GPU core. Hardware mechanisms remain in
//! the existing channel/CE/GR modules; only engine receipts can publish Done.
use alloc::{collections::VecDeque, vec::Vec};
use crate::{gpu::dev::{Done, Reap, Tag}, gpu_completion};
use super::{copy::Rect, gr::{channel::NativeChannel, draw::Draw}, queue::Queue};

#[derive(Clone, Copy)]
pub enum Operation { Copy(Rect), Draw(Draw) }

pub struct Request {
    pub tag: Tag,
    pub fence: Option<u64>,
    operation: Operation,
    resource: u64,
    generation: u64,
    upload: bool,
}
impl Request {
    /// Kernel builders resolve validated handles to GPU addresses before admission.
    pub fn copy(tag: Tag, caller: u64, resource: u64, generation: u64, rect: Rect) -> Self {
        Self { tag, fence: Some(caller), operation: Operation::Copy(rect), resource, generation, upload: false }
    }
    pub fn draw(tag: Tag, caller: u64, resource: u64, generation: u64, draw: Draw) -> Self {
        Self { tag, fence: Some(caller), operation: Operation::Draw(draw), resource, generation, upload: false }
    }
    pub fn upload(tag: Tag, caller: u64, resource: u64, generation: u64, rect: Rect) -> Self {
        let mut request = Self::copy(tag, caller, resource, generation, rect);
        request.upload = true;
        request
    }
    pub fn is_upload(&self) -> bool { self.upload }
}
struct Sent { request: Request, identity: gpu_completion::Request }

pub struct Device {
    channel: NativeChannel,
    board: super::caps::BoardState,
    queue: Queue,
    pending: VecDeque<Sent>,
    fault: bool,
    resetting: Option<Result<(), &'static str>>,
}
impl Device {
    #[allow(dead_code)] // Native device installation uses this public integration API.
    pub(crate) fn new(channel: NativeChannel, board: super::caps::BoardState) -> Self {
        Self { channel, board, queue: Queue::new(), pending: VecDeque::new(), fault: false, resetting: None }
    }
    /// Recheck snapshot identity against the channel on every query. Restart's
    /// new epoch cannot silently inherit old object-creation evidence.
    pub fn caps(&self) -> gpu_abi::Caps {
        super::caps::Profile::from_board(&self.board, self.channel.identity()).record(None)
    }
    pub fn admission(&self, masked: Option<u32>) -> Result<(), (u32, u32)> {
        super::caps::Profile::from_board(&self.board, self.channel.identity()).admission(masked)
    }
    pub fn send(&mut self, request: Request) -> Result<(), Request> {
        if !self.has_room(1) || self.pending.try_reserve(1).is_err() { return Err(request); }
        let Ok(sequence) = self.queue.next() else { return Err(request) };
        let (context, epoch) = self.channel.identity();
        let caller = request.fence.unwrap_or(0);
        if caller == 0 || request.resource == 0 || request.generation == 0 { return Err(request); }
        let identity = gpu_completion::Request { context, epoch, caller, sequence,
            resource: request.resource, generation: request.generation };
        // Retain the request on uncertain publication: a failing doorbell or
        // BAR flush leaves backing in pending until confirmed recovery.
        let result = self.channel.publish(sequence, &request.operation);
        self.pending.push_back(Sent { request, identity });
        if result.is_err() || self.queue.publish(sequence).is_err() { self.fault = true; }
        Ok(())
    }
    pub fn reap(&mut self) -> Reap {
        if self.resetting.is_some() { return Reap::Empty; }
        if self.needs_reset() { return Reap::Fault; }
        let Some(front) = self.pending.front() else { return Reap::Empty };
        let sample = match self.channel.sample(front.identity.sequence) {
            Ok(sample) if sample.healthy => sample,
            _ => { self.fault = true; return Reap::Fault; }
        };
        // No caller fence, constant success tag, GET or host reference alone
        // can construct the opaque engine completion.
        let Some(completed) = front.identity.complete(sample) else { return Reap::Empty };
        let identity = completed.request();
        if self.queue.complete(identity.sequence).is_err() { self.fault = true; return Reap::Fault; }
        let sent = self.pending.pop_front().unwrap();
        Reap::Done(Done { tag: sent.request.tag, fence: Some(identity.caller),
            native_fence: Some(u64::from(identity.sequence)), answer: gpu_abi::gk208::SUCCESS,
            echoed: u64::from(sample.sequence), fenced: true })
    }
    pub fn has_room(&self, segments: usize) -> bool {
        !self.needs_reset() && self.resetting.is_none() && segments <= 4
            && self.pending.len() + segments <= 4 && self.queue.next().is_ok()
    }
    pub fn outstanding(&self) -> usize { self.pending.len() }
    pub fn queue_size(&self) -> usize { 4 }
    pub fn needs_reset(&self) -> bool { self.fault || !self.channel.healthy() }
    pub fn begin_reset(&mut self) {
        if self.resetting.is_none() { self.resetting = Some(self.channel.reset()); }
    }
    pub fn reset_done(&mut self) -> bool { matches!(self.resetting, Some(Ok(()))) }
    pub fn restart(&mut self) -> (Vec<Request>, Result<(), &'static str>) {
        match self.resetting {
            Some(Ok(())) => (),
            Some(Err(error)) => return (Vec::new(), Err(error)),
            None => return (Vec::new(), Err("native reset not confirmed; backing retained")),
        }
        // A restart refusal retains requests as well as channel backing.
        if let Err(error) = self.channel.restart() {
            self.resetting = Some(Err(error));
            self.fault = true;
            return (Vec::new(), Err(error));
        }
        let held = self.pending.drain(..).map(|sent| sent.request).collect();
        self.queue = Queue::new();
        self.fault = false;
        self.resetting = None;
        (held, Ok(()))
    }
}
