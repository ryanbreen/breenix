//! Native requests under the shared GPU core. Hardware mechanisms remain in
//! the existing channel/CE/GR modules. Engine receipts publish fences; storage
//! operations return unfenced acknowledgements after the VM owner completes them.
use alloc::{collections::VecDeque, sync::Arc, vec::Vec};
use crate::{gpu::dev::{Done, Reap, Tag}, gpu_completion};
use super::{copy::Rect, gr::{channel::NativeChannel, draw::Draw}, queue::Queue};

#[derive(Clone, Copy)]
pub enum Operation { Copy(Rect), Transfer(super::copy::Plan), Draw(Draw) }

/// Commands originate only in the validated kernel encoder. The owned storage
/// stays held by pending requests on uncertain publication or failed reset.
pub struct EncodedDraw {
    pub words: Vec<u32>,
    pub target: u32,
    #[cfg(target_os = "none")]
    pub(in crate::gk208) _storage: Arc<super::encode::Storage>,
}
pub enum DrawWork { Triangle(Draw), Encoded(EncodedDraw) }
impl From<Draw> for DrawWork { fn from(d: Draw) -> Self { Self::Triangle(d) } }
impl From<EncodedDraw> for DrawWork { fn from(d: EncodedDraw) -> Self { Self::Encoded(d) } }

pub struct Request {
    pub tag: Tag,
    pub fence: Option<u64>,
    operation: Option<Operation>,
    encoded: Option<EncodedDraw>,
    /// CPU storage/VM acknowledgement, deliberately unfenced.
    storage: bool,
    _backing: Option<Arc<crate::gpu::dma::Pages>>,
    #[cfg(target_os = "none")]
    _transfer_hold: Option<super::ce::readback::TransferHold>,
    resource: u64,
    generation: u64,
    upload: bool,
    #[cfg(any(feature = "testing", feature = "gt710_core_probe"))]
    pub(crate) control: super::core_record::Control,
}
impl Request {
    /// Kernel builders resolve validated handles to GPU addresses before admission.
    pub fn copy(tag: Tag, caller: u64, resource: u64, generation: u64, rect: Rect) -> Self {
        Self { tag, fence: Some(caller), operation: Some(Operation::Copy(rect)), encoded: None, storage: false, _backing: None,
            #[cfg(target_os = "none")]
            _transfer_hold: None, resource, generation, upload: false,
            #[cfg(any(feature = "testing", feature = "gt710_core_probe"))]
            control: super::core_record::Control::Clean }
    }
    pub fn draw(tag: Tag, caller: u64, resource: u64, generation: u64, draw: impl Into<DrawWork>) -> Self {
        let (operation, encoded) = match draw.into() {
            DrawWork::Triangle(draw) => (Some(Operation::Draw(draw)),None),
            DrawWork::Encoded(draw) => (None,Some(draw)),
        };
        Self { tag, fence: Some(caller), operation, encoded, storage: false, _backing: None,
            #[cfg(target_os = "none")]
            _transfer_hold: None, resource, generation, upload: false,
            #[cfg(any(feature = "testing", feature = "gt710_core_probe"))]
            control: super::core_record::Control::Clean }
    }
    pub fn upload(tag: Tag, caller: u64, resource: u64, generation: u64, rect: Rect) -> Self {
        let mut request = Self::copy(tag, caller, resource, generation, rect);
        request.upload = true;
        request
    }
    pub(in crate::gk208) fn transfer(tag: Tag, caller: u64, resource: u64,
        plan: super::copy::Plan, upload: bool,
        #[cfg(target_os = "none")] hold: super::ce::readback::TransferHold,
        backing: Option<Arc<crate::gpu::dma::Pages>>,
    ) -> Self {
        Self { tag, fence: Some(caller), operation: Some(Operation::Transfer(plan)), encoded: None,
            storage: false, _backing: backing,
            #[cfg(target_os = "none")]
            _transfer_hold: Some(hold),
            resource, generation: 1, upload,
            #[cfg(any(feature = "testing", feature = "gt710_core_probe"))]
            control: super::core_record::Control::Clean }
    }
    pub(crate) fn transfer_controls(&mut self, extras: super::copy::Extras) {
        if let Some(Operation::Transfer(plan)) = self.operation.as_mut() {
            *plan = plan.with_extras(extras);
        }
    }
    pub fn is_upload(&self) -> bool { self.upload }
    /// Actual allocation/map/unmap completed in the resource owner before this
    /// receipt was constructed. This is not an engine completion or a fence.
    pub(crate) fn storage(tag: Tag, backing: Option<Arc<crate::gpu::dma::Pages>>) -> Self {
        Self { tag, fence: None, operation: None, encoded: None, storage: true, _backing: backing,
            #[cfg(target_os = "none")]
            _transfer_hold: None,
            resource: 0, generation: 0, upload: false,
            #[cfg(any(feature = "testing", feature = "gt710_core_probe"))]
            control: super::core_record::Control::Clean }
    }
}
struct Sent { request: Request, identity: Option<gpu_completion::Request> }

pub struct Device {
    channel: NativeChannel,
    board: super::caps::BoardState,
    queue: Queue,
    pending: VecDeque<Sent>,
    fault: bool,
    first_error: Option<&'static str>,
    resetting: Option<Result<(), &'static str>>,
}
impl Device {
    pub(crate) fn new(channel: NativeChannel, board: super::caps::BoardState) -> Self {
        Self { channel, board, queue: Queue::new(), pending: VecDeque::new(), fault: false, first_error: None, resetting: None }
    }
    pub(crate) fn registered(channel: NativeChannel, board: super::caps::BoardState, last: u32) -> Self {
        let mut device = Self::new(channel, board);
        device.queue = Queue::drained(last);
        device
    }
    /// Recheck snapshot identity against the channel when querying CAPS. Restart's
    /// new epoch cannot silently inherit old object-creation evidence.
    pub fn caps(&self) -> gpu_abi::Caps {
        super::caps::Profile::from_board(&self.board, self.channel.identity()).record(None)
    }
    #[cfg(target_os = "none")]
    pub(in crate::gk208) fn set_probe_results(&mut self, probes: super::caps::ProbeResults) {
        self.board.probes = probes;
    }
    pub fn admission(&self, masked: Option<u32>) -> Result<(), (u32, u32)> {
        super::caps::Profile::from_board(&self.board, self.channel.identity()).admission(masked)
    }
    pub fn send(&mut self, request: Request) -> Result<(), Request> {
        if !self.has_room(1) || self.pending.try_reserve(1).is_err() { return Err(request); }
        if request.storage {
            self.pending.push_back(Sent { request, identity: None });
            return Ok(());
        }
        if request.operation.is_none() && request.encoded.is_none() { return Err(request); }
        let Ok(sequence) = self.queue.next() else { return Err(request) };
        let (context, epoch) = self.channel.identity();
        let caller = request.fence.unwrap_or(0);
        if caller == 0 || request.resource == 0 || request.generation == 0 { return Err(request); }
        let identity = gpu_completion::Request { context, epoch, caller, sequence,
            resource: request.resource, generation: request.generation };
        // Retain the request on uncertain publication: a failing doorbell or
        // BAR flush leaves backing in pending until confirmed recovery.
        let result = if let Some(draw) = request.encoded.as_ref() {
            self.channel.publish_draw(sequence, draw)
        } else {
            let operation = request.operation.as_ref().unwrap();
            #[cfg(not(any(feature = "testing", feature = "gt710_core_probe")))]
            { self.channel.publish(sequence, operation) }
            #[cfg(any(feature = "testing", feature = "gt710_core_probe"))]
            { if request.control == super::core_record::Control::Clean {
                self.channel.publish(sequence, operation)
            } else { self.channel.publish_probe(sequence, operation, request.control) } }
        };
        self.pending.push_back(Sent { request, identity: Some(identity) });
        if let Err(error) = result.and_then(|()| self.queue.publish(sequence)) {
            self.first_error.get_or_insert(error);
            self.fault = true;
        }
        Ok(())
    }
    pub fn reap(&mut self) -> Reap {
        if self.resetting.is_some() { return Reap::Empty; }
        if self.needs_reset() { return Reap::Fault; }
        let Some(front) = self.pending.front() else { return Reap::Empty };
        let Some(identity) = front.identity else {
            let sent = self.pending.pop_front().unwrap();
            return Reap::Done(Done { tag: sent.request.tag, fence: None, native_fence: None,
                answer: gpu_abi::gk208::SUCCESS, echoed: 0, fenced: false });
        };
        let mut sample = match self.channel.sample(identity.sequence) {
            Ok(sample) if sample.healthy => sample,
            result => {
                self.first_error.get_or_insert(result.err().unwrap_or("native completion binding lost"));
                self.fault = true;
                return Reap::Fault;
            }
        };
        // No caller fence, constant success tag, GET or host reference alone
        // can construct the opaque engine completion.
        let completed = match identity.complete(sample) {
            Some(completed) => completed,
            None if matches!(front.request.operation, Some(Operation::Transfer(_))) => {
                // A bad transfer echo may retire only behind an independent device
                // drain with the same channel/epoch. No control flag certifies it.
                let drain = match self.channel.transfer_drain(identity.sequence) {
                    Ok(sample) if sample.healthy => sample,
                    _ => { self.fault = true; return Reap::Fault; }
                };
                let Some(completed) = identity.complete(drain) else { return Reap::Empty };
                // Re-read after the terminal release, rather than keeping an echo
                // sampled while the CE was still writing this slot.
                sample = match self.channel.sample(identity.sequence) {
                    Ok(sample) if sample.healthy => sample,
                    _ => { self.fault = true; return Reap::Fault; }
                };
                completed
            }
            None => return Reap::Empty,
        };
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
    pub fn fault_reason(&self) -> Option<&'static str> { self.first_error }
    pub fn begin_reset(&mut self) {
        if self.resetting.is_none() {
            #[cfg(target_os = "none")]
            self.channel.record_fault(self.fault_reason().unwrap_or("native health check failed"));
            self.resetting = Some(self.channel.reset());
            #[cfg(target_os = "none")]
            if let Some(Err(error)) = self.resetting {
                crate::serial_println!("GT710_NATIVE_RESET_REFUSAL {}", error);
            }
        }
    }
    pub fn reset_done(&mut self) -> bool { matches!(self.resetting, Some(Ok(()))) }
    /// Requalify the restored channel with new engine observations. The old
    /// epoch's executable results cannot admit a replacement incarnation.
    #[cfg(target_os = "none")]
    pub(crate) fn reprobe(&mut self) -> Result<(), &'static str> {
        self.board.channel = self.channel.identity();
        self.board.probes = super::caps::ProbeResults::new();
        super::caps::run(self)?;
        self.admission(None).map_err(|(_, missing)| gpu_abi::capability::name(missing))
    }
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
        self.first_error = None;
        self.resetting = None;
        (held, Ok(()))
    }
}

#[cfg(any(feature = "testing", feature = "gt710_core_probe"))]
impl Device {
    pub(crate) fn probe(channel: NativeChannel, board: super::caps::BoardState, last: u32) -> Self {
        Self::registered(channel, board, last)
    }
    pub(crate) fn probe_identity(&self) -> Result<(u64, u64, u32), &'static str> {
        let (context, epoch) = self.channel.identity();
        Ok((context, epoch, self.queue.next()?))
    }
    pub(crate) fn probe_sample(&mut self, sequence: u32) -> Result<gpu_completion::CeSample, &'static str> {
        self.channel.sample(sequence)
    }
    /// Negative requests retire without Done only after the separate CE drain
    /// release and FIFO consumption, while their owner retains the mappings.
    pub(crate) fn probe_reject(&mut self) -> Result<Option<gpu_completion::CeSample>, &'static str> {
        let Some(sent) = self.pending.front() else { return Err("core control absent") };
        if !matches!(sent.request.control, super::core_record::Control::WithheldRelease | super::core_record::Control::StaleEpoch) {
            return Err("core request is not a negative control");
        }
        let identity = sent.identity.ok_or("storage receipt is not an engine control")?;
        if !self.channel.probe_drained(identity.sequence)? { return Ok(None); }
        // Resample after the terminal release: an earlier observation could
        // have raced the engine while it was still writing the epoch words.
        let sample = self.channel.sample(identity.sequence)?;
        if identity.complete(sample).is_some() { return Err("core control completed successfully"); }
        let expected = match sent.request.control {
            super::core_record::Control::WithheldRelease => sample.sequence == 0 && sample.epoch == identity.epoch,
            super::core_record::Control::StaleEpoch => sample.sequence == identity.sequence && sample.epoch == identity.epoch.wrapping_sub(1),
            _ => false,
        };
        if !expected || !sample.healthy || sample.context != identity.context || sample.consumed < identity.sequence {
            return Err("core control did not produce the expected hardware effect");
        }
        self.queue.complete(identity.sequence)?;
        self.pending.pop_front();
        Ok(Some(sample))
    }
    pub(crate) fn into_probe_channel(self) -> NativeChannel { self.channel }
}
