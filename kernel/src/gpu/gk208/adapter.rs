//! Native requests under the shared GPU core. Hardware mechanisms remain in
//! the existing channel/CE/GR modules. Engine receipts publish fences; storage
//! operations return unfenced acknowledgements after the VM owner completes them.
use alloc::{collections::VecDeque, sync::Arc, vec::Vec};
use crate::{gpu::dev::{Done, Reap, Tag}, gpu_completion};
use super::{copy::Rect, gr::{channel::NativeChannel, draw::Draw}, queue::Queue};

#[derive(Clone, Copy)]
pub enum Operation { Copy(Rect), Transfer(super::copy::Plan), Draw(Draw), Present, Undefined }

/// Commands originate only in the validated kernel encoder. The owned storage
/// stays held by pending requests on uncertain publication or failed reset.
pub struct EncodedDraw {
    pub words: Vec<u32>,
    pub target: u32,
    pub transfer: Option<super::copy::Plan>,
    #[cfg(target_os = "none")]
    pub command: u32,
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
    scanout: Option<super::present::Surface>,
    #[cfg(any(feature = "testing", feature = "gt710_core_probe"))]
    pub(crate) control: super::core_record::Control,
}
impl Request {
    /// Kernel builders resolve validated handles to GPU addresses before admission.
    pub fn copy(tag: Tag, caller: u64, resource: u64, generation: u64, rect: Rect) -> Self {
        Self { tag, fence: Some(caller), operation: Some(Operation::Copy(rect)), encoded: None, storage: false, _backing: None,
            #[cfg(target_os = "none")]
            _transfer_hold: None, resource, generation, upload: false,
            scanout: None,
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
            scanout: None,
            #[cfg(any(feature = "testing", feature = "gt710_core_probe"))]
            control: super::core_record::Control::Clean }
    }
    /// The control names a live logical context, but allocates no image.
    /// Its report uses the channel's retained receipt page and native sequence.
    pub fn undefined_command(tag: Tag, ctx: u32, caller: u64) -> Option<Self> {
        if ctx == 0 || caller == 0 { return None; }
        Some(Self { tag, fence: Some(caller), operation: Some(Operation::Undefined),
            encoded: None, storage: false, _backing: None,
            #[cfg(target_os = "none")]
            _transfer_hold: None,
            resource: u64::from(ctx), generation: 1, upload: false,
            scanout: None,
            #[cfg(any(feature = "testing", feature = "gt710_core_probe"))]
            control: super::core_record::Control::Clean })
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
            scanout: None,
            #[cfg(any(feature = "testing", feature = "gt710_core_probe"))]
            control: super::core_record::Control::Clean }
    }
    pub(crate) fn transfer_controls(&mut self, extras: super::copy::Extras) {
        if let Some(Operation::Transfer(plan)) = self.operation.as_mut() {
            *plan = plan.with_extras(extras);
        }
    }
    pub(in crate::gk208) fn present(tag:Tag, caller:u64, surface:super::present::Surface,
        #[cfg(target_os = "none")] hold:super::ce::readback::TransferHold,
    ) -> Self {
        Self {tag,fence:Some(caller),operation:Some(Operation::Present),encoded:None,
            storage:false,_backing:None,
            #[cfg(target_os = "none")]
            _transfer_hold:Some(hold),
            resource:u64::from(surface.resource),generation:1,upload:false,scanout:Some(surface),
            #[cfg(any(feature = "testing", feature = "gt710_core_probe"))]
            control:super::core_record::Control::Clean}
    }
    pub fn is_upload(&self) -> bool { self.upload }
    /// Actual allocation/map/unmap completed in the resource owner before this
    /// receipt was constructed. This is not an engine completion or a fence.
    pub(crate) fn storage(tag: Tag, backing: Option<Arc<crate::gpu::dma::Pages>>) -> Self {
        Self { tag, fence: None, operation: None, encoded: None, storage: true, _backing: backing,
            #[cfg(target_os = "none")]
            _transfer_hold: None,
            resource: 0, generation: 0, upload: false,
            scanout: None,
            #[cfg(any(feature = "testing", feature = "gt710_core_probe"))]
            control: super::core_record::Control::Clean }
    }
}
#[cfg(target_os = "none")]
#[derive(Debug)]
struct RequestTrace {
    tag: Tag, channel: u64, epoch: u64, caller: u64, sequence: u32,
    kind: &'static str, target: u64, frame: u32, address: u64, command: u32,
}

struct Sent { request: Request, identity: Option<gpu_completion::Request>, answer: Option<u32>,
    #[cfg(target_os = "none")]
    execution: Option<super::cost::Execution>,
}

pub struct Device {
    channel: NativeChannel,
    board: super::caps::BoardState,
    queue: Queue,
    pending: VecDeque<Sent>,
    retired: VecDeque<(Request, Option<u32>)>,
    attributed: Option<(super::gr::error::Trap, u32)>,
    #[cfg(target_os = "none")]
    recent: VecDeque<RequestTrace>,
    pri_pending: bool,
    fault: bool,
    first_error: Option<&'static str>,
    resetting: Option<Result<(), &'static str>>,
}
impl Device {
    #[cfg(target_os="none")]
    pub(crate) fn heap_footprint(&self) -> [usize; 4] {
        let channel = self.channel.heap_footprint();
        [self.pending.capacity() * core::mem::size_of::<Sent>()
            + self.recent.capacity() * core::mem::size_of::<RequestTrace>(),
            self.retired.capacity() * core::mem::size_of::<(Request, Option<u32>)>(), channel[0], channel[1]]
    }
    pub(crate) fn new(channel: NativeChannel, board: super::caps::BoardState) -> Self {
        Self { channel, board, queue: Queue::new(), pending: VecDeque::new(),
            retired: VecDeque::new(), attributed: None,
            #[cfg(target_os = "none")]
            recent: VecDeque::new(), pri_pending: false, fault: false, first_error: None, resetting: None }
    }
    pub(crate) fn registered(channel: NativeChannel, board: super::caps::BoardState, last: u32) -> Self {
        let mut device = Self::new(channel, board);
        device.queue = Queue::drained(last);
        device
    }
    #[cfg(target_os = "none")]
    pub(crate) fn terminal_identity(&self) -> Option<(u64, u64)> {
        (!self.needs_reset()).then(|| self.channel.terminal_identity()).flatten()
    }
    /// Recheck snapshot identity against the channel when querying CAPS. Restart's
    /// new epoch cannot silently inherit old object-creation evidence.
    #[cfg(target_os = "none")]
    pub(crate) fn terminal_queue(&self) -> Result<super::emergency::Queue, &'static str> {
        if self.needs_reset() { return Err("terminal channel needs reset"); }
        self.channel.terminal_queue()
    }
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
        #[cfg(target_os = "none")]
        let _cost = super::cost::phase(super::cost::Slot::NativeSubmit);
        if !self.has_room(1) || self.pending.try_reserve(1).is_err() { return Err(request); }
        #[cfg(target_os = "none")]
        if self.recent.len() < 16 && self.recent.try_reserve(1).is_err() { return Err(request); }
        if request.storage {
            self.pending.push_back(Sent { request, identity: None, answer: None,
                #[cfg(target_os = "none")]
                execution: None,
            });
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
            if let Some(plan) = draw.transfer {
                self.channel.publish(sequence, &Operation::Transfer(plan))
            } else {
                self.channel.publish_draw(sequence, draw)
            }
        } else {
            let operation = request.operation.as_ref().unwrap();
            #[cfg(not(any(feature = "testing", feature = "gt710_core_probe")))]
            { self.channel.publish(sequence, operation) }
            #[cfg(any(feature = "testing", feature = "gt710_core_probe"))]
            { if request.control == super::core_record::Control::Clean {
                self.channel.publish(sequence, operation)
            } else { self.channel.publish_probe(sequence, operation, request.control) } }
        };
        if result.is_err() && !self.channel.published(sequence) {
            return Err(request);
        }
        #[cfg(target_os = "none")]
        {
            let (kind, frame, address, command) = request.encoded.as_ref().map_or_else(
                || (match request.operation { Some(Operation::Copy(_)) => "copy", Some(Operation::Transfer(_)) => "transfer", Some(Operation::Undefined) => "unmapped-write", Some(Operation::Present) => "present", _ => "triangle" }, 0, 0, 0),
                |d| ("encoded-draw", d._storage.resource, d._storage.address, d.command));
            if self.recent.len() == 16 { self.recent.pop_front(); }
            self.recent.push_back(RequestTrace { tag: request.tag, channel: context, epoch, caller, sequence,
                kind, target: request.resource, frame, address, command });
        }
        #[cfg(target_os = "none")]
        let execution = if result.is_ok() {
            request.encoded.as_ref().and_then(|d| super::cost::execution(
                if d.transfer.is_some() { super::cost::Slot::ScratchCopy }
                else { super::cost::Slot::DrawExecute }))
        } else { None };
        self.pending.push_back(Sent { request, identity: Some(identity), answer: None,
            #[cfg(target_os = "none")]
            execution,
        });
        if let Err(error) = result.and_then(|()| self.queue.publish(sequence)) {
            self.first_error.get_or_insert(error);
            self.fault = true;
        }
        Ok(())
    }
    pub fn reap(&mut self) -> Reap {
        if self.resetting.is_some() { return Reap::Empty; }
        if let Some((request, answer)) = self.retired.pop_front() {
            return match answer {
                Some(answer) => Reap::Done(Done { tag: request.tag, fence: None,
                    native_fence: None, answer, echoed: 0, fenced: false }),
                None => Reap::Reset(request.tag),
            };
        }
        if self.attributed.is_some() || self.pri_pending { return Reap::Empty; }
        if self.fault { return Reap::Fault; }
        // Handle an attributed event before checking health or sampling
        // earlier work on the channel. IRQ top halves do not call this reaper.
        if let Err(reason) = self.retire_error_trap() {
            if reason == "GR PRI pending" { self.pri_pending = true; return Reap::Empty; }
            self.first_error.get_or_insert(reason);
            self.fault = true;
            return Reap::Fault;
        }
        if self.attributed.is_some() { return Reap::Empty; }
        if let Err(reason) = self.channel.healthy() {
            return self.retry_error_trap(reason);
        }
        let Some(front) = self.pending.front() else { return Reap::Empty };
        let Some(identity) = front.identity else {
            let sent = self.pending.pop_front().unwrap();
            return Reap::Done(Done { tag: sent.request.tag, fence: None, native_fence: None,
                answer: gpu_abi::gk208::SUCCESS, echoed: 0, fenced: false });
        };
        let mut sample = match self.channel.sample(identity.sequence) {
            Ok(sample) if sample.healthy => sample,
            result => {
                return self.retry_error_trap(result.err().unwrap_or("native completion binding lost"));
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
                    result => return self.retry_error_trap(result.err().unwrap_or("native transfer binding lost")),
                };
                let Some(completed) = identity.complete(drain) else { return Reap::Empty };
                // Re-read after the terminal release, rather than keeping an echo
                // sampled while the CE was still writing this slot.
                sample = match self.channel.sample(identity.sequence) {
                    Ok(sample) if sample.healthy => sample,
                    result => return self.retry_error_trap(result.err().unwrap_or("native transfer binding lost")),
                };
                completed
            }
            None => return Reap::Empty,
        };
        if matches!(front.request.operation, Some(Operation::Undefined)) && front.answer.is_none() {
            self.first_error.get_or_insert("unmapped write completed without an MMU fault");
            self.fault = true;
            return Reap::Fault;
        }
        if let Some(surface) = front.request.scanout {
            if let Err(reason) = self.channel.present(surface) {
                self.first_error.get_or_insert(reason);
                self.fault = true;
                return Reap::Fault;
            }
        }
        let identity = completed.request();
        if self.queue.complete(identity.sequence).is_err() { self.fault = true; return Reap::Fault; }
        let sent = self.pending.pop_front().unwrap();
        #[cfg(target_os = "none")]
        if let Some(execution) = sent.execution { execution.finish(); }
        Reap::Done(Done { tag: sent.request.tag, fence: Some(identity.caller),
            native_fence: Some(u64::from(identity.sequence)), answer: sent.answer.unwrap_or(gpu_abi::gk208::SUCCESS),
            echoed: u64::from(sample.sequence), fenced: true })
    }
    // A trap can arrive between capture and the health/completion MMIO reads.
    // Acknowledging that attributed event defers sampling to the next poll;
    // other health failures retain the original confirmed-reset path.
    fn retry_error_trap(&mut self, reason: &'static str) -> Reap {
        if reason == "GR PRI pending" { self.pri_pending = true; return Reap::Empty; }
        match self.retire_error_trap() {
            Ok(true) => Reap::Empty,
            Err("GR PRI pending") => { self.pri_pending = true; Reap::Empty }
            result => {
                self.first_error.get_or_insert(result.err().unwrap_or(reason));
                self.fault = true;
                Reap::Fault
            }
        }
    }
    fn retire_error_trap(&mut self) -> Result<bool, &'static str> {
        let Some(trap) = self.channel.error_trap()? else { return Ok(false); };
        let (context, epoch) = self.channel.identity();
        let sent = self.pending.iter_mut().find(|sent| {
            matches!(sent.request.operation, Some(Operation::Undefined)) && sent.answer.is_none()
                && sent.identity.is_some_and(|i| i.context == context && i.epoch == epoch
                    && trap.instance == i.context && trap.page == i.sequence % 32768)
        }).ok_or("unattributed MMU write; backing retained")?;
        sent.answer = Some(trap.status);
        self.attributed = Some((trap, sent.identity.unwrap().sequence));
        Ok(true)
    }
    pub fn has_room(&self, segments: usize) -> bool {
        !self.pri_pending && !self.needs_reset() && self.attributed.is_none() && self.retired.is_empty()
            && self.resetting.is_none() && segments <= 4
            && self.pending.len() + segments <= 4 && self.queue.next().is_ok()
    }
    pub fn outstanding(&self) -> usize { self.pending.len() + self.retired.len() }
    pub fn queue_size(&self) -> usize { 4 }
    pub fn needs_reset(&self) -> bool {
        self.fault || (!self.pri_pending && self.attributed.is_none()
            && matches!(self.channel.healthy(), Err(reason) if reason != "GR PRI pending"))
    }
    pub(crate) fn recovery_pending(&self) -> bool {
        (self.pri_pending || self.attributed.is_some()) && self.resetting.is_none()
    }
    /// Called only after kgpu has borrowed this adapter outside CORE.
    pub(crate) fn retry_pri(&mut self) {
        if !self.pri_pending { return; }
        self.pri_pending = false;
        if let Err(reason) = self.channel.wait_pri() {
            self.first_error.get_or_insert(reason);
            self.fault = true;
        }
    }
    pub fn fault_reason(&self) -> Option<&'static str> { self.first_error }
    /// The worker owns the device outside CORE here. Capture once before the
    /// reset, rather than from reap(), which runs while CORE is locked.
    pub fn begin_reset(&mut self) {
        if self.resetting.is_none() {
            self.pri_pending = false;
            #[cfg(target_os = "none")]
            if self.attributed.is_none() {
                self.channel.record_fault(self.fault_reason().or_else(|| self.channel.healthy().err())
                    .unwrap_or("shared core requested reset"));
                for t in &self.recent {
                    let pending = self.pending.iter().any(|s| s.identity.is_some_and(|i| i.sequence == t.sequence && i.epoch == t.epoch));
                    crate::serial_println!("GT710_NATIVE_REQUEST pending={} tag={:?} channel={:#x} epoch={} caller={} sequence={} kind={} target={} frame={} address={:#x} command={}",
                        pending,t.tag,t.channel,t.epoch,t.caller,t.sequence,t.kind,t.target,t.frame,t.address,t.command);
                }
            }
            self.resetting = Some(match self.attributed {
                Some((trap, sequence)) => self.channel.acknowledge_error(trap, sequence)
                    .and_then(|()| self.channel.reset()),
                None => self.channel.reset(),
            });
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
        let epoch = self.board.channel.1;
        let result = super::caps::run_reprobe(self, epoch)
            .and_then(|()| self.admission(None).map_err(|(_, missing)| gpu_abi::capability::name(missing)));
        if result.is_err() { self.channel.record_refusal()?; }
        result
    }
    pub fn restart(&mut self) -> (Vec<Request>, Result<(), &'static str>) {
        match self.resetting {
            Some(Ok(())) => (),
            Some(Err(error)) => return (Vec::new(), Err(error)),
            None => return (Vec::new(), Err("native reset not confirmed; backing retained")),
        }
        if self.attributed.is_some() && self.retired.try_reserve(self.pending.len()).is_err() {
            return (Vec::new(), Err("native retirement allocation; backing retained"));
        }
        let mut held = Vec::new();
        if self.attributed.is_none() && held.try_reserve_exact(self.pending.len()).is_err() {
            return (held, Err("native restart allocation; backing retained"));
        }
        // A restart refusal retains requests as well as channel backing.
        if let Err(error) = self.channel.restart() {
            self.resetting = Some(Err(error));
            self.fault = true;
            return (Vec::new(), Err(error));
        }
        let held = if let Some((_, sequence)) = self.attributed.take() {
            // Publish the producing error before reset outcomes for earlier
            // chunks of the same submission can obscure it in the core.
            let index = self.pending.iter().position(|s| s.identity.is_some_and(|i| i.sequence == sequence))
                .expect("retained attributed request");
            let sent = self.pending.remove(index).unwrap();
            self.retired.push_back((sent.request, sent.answer));
            for sent in self.pending.drain(..) {
                // No replay of potentially executed peer writes. Only storage
                // acknowledgements and the attributed error have an answer.
                let answer = sent.answer.or_else(|| sent.request.storage.then_some(gpu_abi::gk208::SUCCESS));
                self.retired.push_back((sent.request, answer));
            }
            Vec::new()
        } else { held.extend(self.pending.drain(..).map(|sent| sent.request)); held };
        self.queue = Queue::new();
        self.fault = false;
        self.first_error = None;
        self.resetting = None;
        (held, Ok(()))
    }
    /// kgpu owns the adapter outside CORE for the entire engine recovery.
    #[cfg(target_os = "none")]
    pub(crate) fn recover_fault(&mut self) -> Result<(), &'static str> {
        if !self.recovery_pending() { return Err("native attributed recovery absent"); }
        if self.pri_pending {
            self.retry_pri();
            // Restore the adapter to CORE before its normal reaper observes
            // a persistent refusal and requests confirmed reset.
            return Ok(());
        }
        self.begin_reset();
        let (held, result) = self.restart();
        debug_assert!(held.is_empty());
        if let Err(error) = result {
            self.channel.record_refusal()?;
            return Err(error);
        }
        // Fresh capability probes must consume only their own receipts. Keep
        // old terminal requests held until restart AND requalification finish.
        let mut retired = core::mem::take(&mut self.retired);
        let result = self.reprobe();
        retired.append(&mut self.retired);
        self.retired = retired;
        result
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
        self.retry_pri();
        self.channel.sample_wait(sequence)
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
        let sample = self.channel.sample_wait(identity.sequence)?;
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
