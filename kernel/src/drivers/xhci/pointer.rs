//! HID pointer interrupt-IN pipes, configured from published USB/HID/xHCI formats.
use super::{spec, Controller, Dma, Ring, Trb};
use crate::drivers::hid_boot::{interface::PointerInterface, pointer::Layout};
use alloc::{string::String, vec::Vec};
use spin::Mutex;

/// One attached pointer interface, as `/proc/xhci/pointers` lists it.
struct Attached {
    port: u8,
    slot: u8,
    interface: u8,
    endpoint: u8,
    axes: &'static str,
    descriptor: Vec<u8>,
    /// The interface's last reports from the device, oldest first, kept by
    /// measurement kernels for the replay control (`replay`).
    #[cfg(feature = "usb_pointer_replay")]
    recorded: alloc::collections::VecDeque<Vec<u8>>,
}

/// Reports kept per interface for replay.
#[cfg(feature = "usb_pointer_replay")]
const RECORDED: usize = 16;

/// Written by the xHCI worker at attach and detach; read by procfs.
static ATTACHED: Mutex<Vec<Attached>> = Mutex::new(Vec::new());

/// `/proc/xhci/pointers`: one line per attached pointer interface, with the
/// axis kinds its report descriptor declares and the descriptor in hex; in a
/// measurement kernel also `recorded=`, the device's last reports in hex,
/// oldest first, comma-separated.
pub fn describe() -> String {
    use core::fmt::Write;
    let mut text = String::new();
    for p in ATTACHED.lock().iter() {
        let _ = write!(
            text,
            "pointer port={} slot={} interface={} endpoint={:#x} axes={} descriptor=",
            p.port, p.slot, p.interface, p.endpoint, p.axes
        );
        for byte in &p.descriptor {
            let _ = write!(text, "{byte:02x}");
        }
        #[cfg(feature = "usb_pointer_replay")]
        {
            text.push_str(" recorded=");
            for (i, report) in p.recorded.iter().enumerate() {
                if i != 0 {
                    text.push(',');
                }
                for byte in report {
                    let _ = write!(text, "{byte:02x}");
                }
            }
        }
        text.push('\n');
    }
    text
}

/// The device on `port` is gone: its interfaces leave the list.
pub(super) fn detached(port: u8) {
    ATTACHED.lock().retain(|p| p.port != port);
}

pub(super) struct Pipe {
    pub dci: u8,
    endpoint: u8,
    ring: Ring,
    buffer: Dma,
    request: u32,
    /// Bytes between the buffer's report slots (`spec::posted_stride`).
    stride: usize,
    posted: spec::Posted,
    layout: Option<Layout>,
    buttons: u8,
    errors: u8,
}

impl Controller {
    pub(super) fn attach_pointers(&mut self, index: usize) -> Result<(), &'static str> {
        let mut interfaces = Vec::new();
        crate::drivers::hid_boot::interface::pointers(&self.devices[index].config, &mut |p| {
            interfaces.push(p)
        })?;
        if self.devices[index].keyboard.is_none() {
            if let Some(interface) = interfaces.first() {
                if self.control(
                    index,
                    spec::set_configuration(interface.configuration),
                    None,
                )? != 1
                {
                    return Err("pointer SET_CONFIGURATION refused");
                }
            }
        }
        for interface in interfaces {
            // A command error can leave the controller owning device.input.
            // Do not rewrite that shared DMA context for another interface.
            self.configure_pointer(index, interface)?;
        }
        Ok(())
    }

    fn configure_pointer(
        &mut self,
        index: usize,
        hid: PointerInterface,
    ) -> Result<(), &'static str> {
        let dci = (hid.endpoint & 15) * 2 + 1;
        if self.devices[index].pointers.iter().any(|p| p.dci == dci) {
            return Ok(());
        }
        // Classify the report before configuring an interrupt endpoint. An
        // auxiliary HID interface must not leave an unpolled endpoint behind.
        let mut device = self.devices.remove(index);
        let setup = spec::setup(0x81, 6, 0x2200, hid.interface as u16, hid.report_length);
        let descriptor =
            self.read_descriptor(&mut device, &|_| setup, hid.report_length, hid.report_length);
        self.devices.insert(index, device);
        let descriptor = descriptor?;
        let layout = match Layout::parse(&descriptor) {
            Ok(layout) => layout,
            Err("HID descriptor has no pointer fields") => return Ok(()),
            Err(error) => return Err(error),
        };
        let interval = spec::interrupt_interval(self.devices[index].speed, hid.interval)?;
        let stride = spec::posted_stride(hid.max_packet as u32, 4096)?;
        let ring = Ring::new(self.address64)?;
        let buffer = match Dma::new(4096, self.address64) {
            Ok(buffer) => buffer,
            Err(error) => {
                ring.memory.release();
                return Err(error);
            }
        };
        let burst = if self.devices[index].speed >= 480_000_000 {
            hid.extra_transactions
        } else {
            0
        };
        let device = &self.devices[index];
        for at in (0..33 * self.contexts.stride).step_by(4) {
            device.input.put32(at, 0);
        }
        device.input.put32(4, 1 | 1 << dci);
        let slot_at = self.contexts.input(0);
        for i in 0..3 {
            device.input.put32(
                slot_at + i * 4,
                device.output.get32(self.contexts.output(0) + i * 4),
            );
        }
        let entries = (device.input.get32(slot_at) >> 27).max(dci as u32);
        device.input.put32(
            slot_at,
            (device.input.get32(slot_at) & !(31 << 27)) | entries << 27,
        );
        let ep_at = self.contexts.input(dci as usize);
        for (i, word) in
            spec::interrupt_in_context(interval, hid.max_packet, burst, ring.memory.phys)
                .iter()
                .enumerate()
        {
            device.input.put32(ep_at + i * 4, *word);
        }
        let slot = device.slot;
        let port = device.port;
        match self.command_code(spec::configure_endpoint(device.input.phys, slot)) {
            Ok(1) => {}
            Ok(_) => {
                ring.memory.release();
                buffer.release();
                return Err("pointer Configure Endpoint refused");
            }
            Err(error) => {
                // A timed-out command may still own the ring. Keep it in the
                // device until teardown disables the slot before freeing DMA.
                self.devices[index].pointers.push(Pipe {
                    dci,
                    endpoint: hid.endpoint,
                    ring,
                    buffer,
                    request: hid.max_packet as u32,
                    stride,
                    posted: spec::Posted::new(),
                    layout: Some(layout),
                    buttons: 0,
                    errors: 0,
                });
                return Err(error);
            }
        }
        self.devices[index].pointers.push(Pipe {
            dci,
            endpoint: hid.endpoint,
            ring,
            buffer,
            request: hid.max_packet as u32,
            stride,
            posted: spec::Posted::new(),
            layout: Some(layout),
            buttons: 0,
            errors: 0,
        });
        // Stay in Report protocol, including wheel/absolute axes and report IDs.
        // Boot mouse protocol's fixed three bytes cannot describe a tablet.
        self.queue_pointer(index, self.devices[index].pointers.len() - 1);
        let axes = self.devices[index].pointers.last().and_then(|p| p.layout.as_ref()).map_or("none", Layout::axes);
        log::info!("[usb] HID pointer attached: port={port} slot={slot} interface={} endpoint={:#x} axes={axes} report-descriptor={} bytes", hid.interface, hid.endpoint, hid.report_length);
        ATTACHED.lock().push(Attached {
            port,
            slot,
            interface: hid.interface,
            endpoint: hid.endpoint,
            axes,
            descriptor,
            #[cfg(feature = "usb_pointer_replay")]
            recorded: alloc::collections::VecDeque::new(),
        });
        Ok(())
    }

    /// Post an interrupt-IN TD in each free report slot (`spec::POSTED_TDS`).
    fn queue_pointer(&mut self, index: usize, pipe_index: usize) {
        let device = &mut self.devices[index];
        let pipe = &mut device.pointers[pipe_index];
        let free: Vec<usize> = pipe.posted.free().collect();
        if free.is_empty() {
            return;
        }
        for buffer_slot in free {
            let at = pipe.buffer.phys + (buffer_slot * pipe.stride) as u64;
            let pointer = pipe.ring.push(spec::normal_in(at, pipe.request));
            pipe.posted.post(buffer_slot, pointer);
        }
        let (slot, dci) = (device.slot, pipe.dci);
        self.ring_doorbell(slot, dci as u32);
    }

    pub(super) fn dispatch_pointer(&mut self, event: Trb) -> bool {
        let dci = ((event.control >> 16) & 31) as u8;
        let Some((index, pipe_index)) = self.devices.iter().enumerate().find_map(|(i, d)| {
            if d.slot != event.slot() {
                return None;
            }
            d.pointers
                .iter()
                .position(|p| p.dci == dci && p.posted.slot(event.parameter & !15).is_some())
                .map(|p| (i, p))
        }) else {
            return false;
        };
        let pipe = &mut self.devices[index].pointers[pipe_index];
        let Some(buffer_slot) = pipe.posted.complete(event.parameter & !15) else {
            return true;
        };
        if matches!(event.code(), 1 | 13) && (event.status & 0xff_ffff) <= pipe.request {
            pipe.errors = 0;
            let data = pipe.buffer.copy(
                buffer_slot * pipe.stride,
                (pipe.request - (event.status & 0xff_ffff)) as usize,
            );
            #[cfg(feature = "usb_pointer_replay")]
            let replayed = self.replay_at.take_if(|(_, at)| *at == event.parameter & !15);
            #[cfg(feature = "usb_pointer_replay")]
            if replayed.is_none() {
                record(self.devices[index].port, self.devices[index].pointers[pipe_index].endpoint, &data);
            }
            let delivered = self.deliver_pointer(index, pipe_index, &data);
            if let Err(error) = delivered {
                log::warn!("[hid] pointer report refused: {error}");
            }
            self.queue_pointer(index, pipe_index);
            #[cfg(feature = "usb_pointer_replay")]
            if let Some((seq, _)) = replayed {
                let armed = self.devices[index].pointers[pipe_index].posted.full();
                replay::answer(seq, delivered.map(|delivered| delivered as u64 | (armed as u64) << 1));
            }
        } else if !matches!(event.code(), 26..=28) {
            pipe.errors += 1;
            if pipe.errors > 3 {
                self.teardown(index, "pointer interrupt-IN errors");
                return true;
            }
            let (slot, endpoint) = (
                self.devices[index].slot,
                self.devices[index].pointers[pipe_index].endpoint,
            );
            let recovered = self.reset_endpoint(slot, dci).and_then(|()| {
                if self.control(index, spec::clear_endpoint_halt(endpoint), None)? != 1 {
                    return Err("pointer CLEAR_FEATURE refused");
                }
                let pipe = &mut self.devices[index].pointers[pipe_index];
                // The dequeue moves past the posted TDs, not only the failed one.
                pipe.posted.abandon();
                let next = pipe.ring.enqueue();
                self.set_dequeue(slot, dci, next)
            });
            if recovered.is_ok() {
                self.queue_pointer(index, pipe_index);
            } else {
                self.teardown(index, "pointer interrupt-IN recovery failed");
            }
        }
        true
    }

    /// A completed report's path to the input core: decode it with the
    /// interface's layout, merge the device's buttons across its interfaces,
    /// and report the motion. Returns whether the report carried pointer data.
    fn deliver_pointer(
        &mut self,
        index: usize,
        pipe_index: usize,
        data: &[u8],
    ) -> Result<bool, &'static str> {
        let pipe = &self.devices[index].pointers[pipe_index];
        let Some(mut motion) = pipe
            .layout
            .as_ref()
            .ok_or("pointer has no report layout")?
            .decode(data)?
        else {
            return Ok(false);
        };
        if let Some(buttons) = motion.buttons {
            self.devices[index].pointers[pipe_index].buttons = buttons as u8;
            motion.buttons = Some(
                self.devices[index]
                    .pointers
                    .iter()
                    .fold(0u32, |held, pipe| held | pipe.buttons as u32),
            );
        }
        crate::input::pointer::report(self.devices[index].port, motion);
        Ok(true)
    }

    /// Replay one report on the pointer interface whose interrupt-IN endpoint
    /// is `endpoint` on root port `port`, through the path a device's report
    /// takes: a Transfer Event for a posted TD, the report read from that TD's
    /// buffer, decoded, and the pipe's TDs posted again. The endpoint is stopped
    /// first (4.6.9), so the controller has given its TDs back; the oldest TD it
    /// did not complete takes the report, and the endpoint's dequeue moves past
    /// it (4.6.10). The event is dispatched with the controller's own; `dispatch`
    /// answers request `seq`.
    #[cfg(feature = "usb_pointer_replay")]
    pub(super) fn replay_pointer(
        &mut self,
        seq: u64,
        port: u8,
        endpoint: u8,
        data: &[u8],
    ) -> Result<(), &'static str> {
        let (index, pipe_index) = self
            .devices
            .iter()
            .enumerate()
            .find_map(|(index, device)| {
                (device.port == port)
                    .then(|| device.pointers.iter().position(|p| p.endpoint == endpoint))
                    .flatten()
                    .map(|pipe_index| (index, pipe_index))
            })
            .ok_or("no attached pointer interface on that port and endpoint")?;
        let (slot, dci, request) = {
            let pipe = &self.devices[index].pointers[pipe_index];
            (self.devices[index].slot, pipe.dci, pipe.request)
        };
        if data.is_empty() || data.len() > request as usize {
            return Err("replayed report does not fit the endpoint's request");
        }
        match self.command_code(spec::stop_endpoint(slot, dci))? {
            1 | 19 => {}
            _ => return Err("Stop Endpoint refused"),
        }
        // The TD the controller was working on reports Stopped (26-28); it
        // goes back to software with the others. Completions it posted before
        // stopping stay queued for `dispatch`.
        self.completions.retain(|e| {
            !(e.kind() == 32
                && e.slot() == slot
                && (e.control >> 16) & 31 == dci as u32
                && matches!(e.code(), 26..=28))
        });
        let completed: Vec<u64> = self
            .completions
            .iter()
            .filter(|e| e.kind() == 32 && e.slot() == slot && (e.control >> 16) & 31 == dci as u32)
            .map(|e| e.parameter & !15)
            .collect();
        let pipe = &mut self.devices[index].pointers[pipe_index];
        let ring = pipe.ring.memory.phys;
        // The oldest TD still posted and uncompleted is at the dequeue pointer.
        let oldest = (0..spec::POSTED_TDS)
            .filter_map(|buffer_slot| {
                let at = pipe.posted.tds(buffer_slot)?;
                (!completed.contains(&at)).then_some((buffer_slot, at))
            })
            .min_by_key(|(_, at)| spec::ring_age(ring, pipe.ring.enqueue(), *at));
        let (buffer_slot, at) = oldest.ok_or("no posted TD to replay into")?;
        for (i, byte) in data.iter().enumerate() {
            pipe.buffer.put8(buffer_slot * pipe.stride + i, *byte);
        }
        let index_of = ((at - ring) / 16) as usize;
        let cycle = pipe.ring.memory.get32(index_of * 16 + 12) & 1 != 0;
        let next = spec::ring_next(ring, index_of, cycle);
        self.set_dequeue(slot, dci, next)?;
        let residual = request - data.len() as u32;
        let code = if residual == 0 { 1u32 } else { 13 };
        self.replay_at = Some((seq, at));
        self.completions.push_back(Trb::new(
            32,
            at,
            code << 24 | residual,
            (slot as u32) << 24 | (dci as u32) << 16,
        ));
        Ok(())
    }
}

/// Keep a report the device sent, for replay (measurement kernels only).
#[cfg(feature = "usb_pointer_replay")]
fn record(port: u8, endpoint: u8, data: &[u8]) {
    if let Some(p) = ATTACHED.lock().iter_mut().find(|p| p.port == port && p.endpoint == endpoint) {
        if p.recorded.len() == RECORDED {
            p.recorded.pop_front();
        }
        p.recorded.push_back(data.to_vec());
    }
}

impl Pipe {
    pub(super) fn release(self) -> usize {
        self.ring.memory.release() + self.buffer.release()
    }
}

/// The pointer measurement's test control for an axis kind the target's host
/// cannot send (Parallels' absolute interface, VMware's relative one):
/// `ioctl(fd, POINTER_REPLAY, &ReplayRequest)` hands one report, normally one
/// the device sent earlier (`recorded=` in `/proc/xhci/pointers`), to the xHCI
/// worker, which completes a posted TD with it (`Controller::replay_pointer`).
/// Built only with the `usb_pointer_replay` feature (kernel/Cargo.toml), which
/// release builds leave off.
/// One caller at a time: a second request replaces an unanswered first.
#[cfg(feature = "usb_pointer_replay")]
pub mod replay {
    use super::super::{request, REPLAY};
    use crate::syscall::{errno, userptr, SyscallResult};
    use alloc::vec::Vec;
    use core::sync::atomic::{AtomicU64, Ordering};
    use spin::Mutex;

    pub const POINTER_REPLAY: u64 = 0xB813_0006;
    const REPORT_MAX: usize = 64;

    /// The ioctl argument: the interface's root port and interrupt-IN endpoint
    /// address (as `/proc/xhci/pointers` lists them) and one report.
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct ReplayRequest {
        port: u32,
        endpoint: u32,
        len: u32,
        report: [u8; REPORT_MAX],
    }

    /// The pending request, taken by the worker: sequence, port, endpoint, report.
    pub(in super::super) static PENDING: Mutex<Option<(u64, u8, u8, Vec<u8>)>> = Mutex::new(None);
    static ASKED: AtomicU64 = AtomicU64::new(0);
    static DONE: AtomicU64 = AtomicU64::new(0);
    /// The last answer: bits 0-1 the worker's result, bit 2 refused.
    static ANSWER: AtomicU64 = AtomicU64::new(0);

    pub fn ioctl(arg: u64) -> SyscallResult {
        let asked = match userptr::copy_from_user(arg as *const ReplayRequest) {
            Ok(asked) => asked,
            Err(error) => return SyscallResult::Err(error),
        };
        if asked.len == 0 || asked.len as usize > REPORT_MAX || asked.port > 255 || asked.endpoint > 255 {
            return SyscallResult::Err(errno::EINVAL as u64);
        }
        let seq = ASKED.fetch_add(1, Ordering::AcqRel) + 1;
        *PENDING.lock() = Some((
            seq,
            asked.port as u8,
            asked.endpoint as u8,
            asked.report[..asked.len as usize].to_vec(),
        ));
        request(&REPLAY);
        let (s, ns) = crate::time::get_monotonic_time_ns();
        let end = s * 1_000_000_000 + ns + 1_000_000_000;
        while DONE.load(Ordering::Acquire) < seq {
            let (s, ns) = crate::time::get_monotonic_time_ns();
            if s * 1_000_000_000 + ns >= end {
                return SyscallResult::Err(errno::ETIMEDOUT as u64);
            }
            crate::task::scheduler::yield_current();
        }
        match ANSWER.load(Ordering::Acquire) {
            answer if answer & 4 != 0 => SyscallResult::Err(errno::ENODEV as u64),
            answer => SyscallResult::Ok(answer),
        }
    }

    /// The worker's answer to request `seq`.
    pub(in super::super) fn answer(seq: u64, result: Result<u64, &'static str>) {
        if let Err(error) = result {
            log::warn!("[usb] pointer replay refused: {error}");
        }
        ANSWER.store(result.unwrap_or(4), Ordering::Release);
        DONE.store(seq, Ordering::Release);
    }
}
