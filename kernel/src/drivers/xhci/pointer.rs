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
}

/// Written by the xHCI worker at attach and detach; read by procfs.
static ATTACHED: Mutex<Vec<Attached>> = Mutex::new(Vec::new());

/// `/proc/xhci/pointers`: one line per attached pointer interface, with the
/// axis kinds its report descriptor declares and the descriptor in hex.
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
    outstanding: Option<u64>,
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
        let mut failure = None;
        for interface in interfaces {
            if let Err(error) = self.configure_pointer(index, interface) {
                log::warn!(
                    "[usb] pointer interface {} refused: {error}",
                    interface.interface
                );
                failure = Some(error);
            }
        }
        failure.map_or(Ok(()), Err)
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
        let descriptor = self.read_descriptor(&mut device, setup, hid.report_length);
        self.devices.insert(index, device);
        let descriptor = descriptor?;
        let layout = match Layout::parse(&descriptor) {
            Ok(layout) => layout,
            Err("HID descriptor has no pointer fields") => return Ok(()),
            Err(error) => return Err(error),
        };
        let interval = spec::interrupt_interval(self.devices[index].speed, hid.interval)?;
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
                    outstanding: None,
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
            outstanding: None,
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
        });
        Ok(())
    }

    fn queue_pointer(&mut self, index: usize, pipe_index: usize) {
        let device = &mut self.devices[index];
        let pipe = &mut device.pointers[pipe_index];
        pipe.outstanding = Some(
            pipe.ring
                .push(spec::normal_in(pipe.buffer.phys, pipe.request)),
        );
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
                .position(|p| p.dci == dci && p.outstanding == Some(event.parameter & !15))
                .map(|p| (i, p))
        }) else {
            return false;
        };
        let pipe = &mut self.devices[index].pointers[pipe_index];
        pipe.outstanding = None;
        if matches!(event.code(), 1 | 13) && (event.status & 0xff_ffff) <= pipe.request {
            pipe.errors = 0;
            let data = pipe
                .buffer
                .copy(0, (pipe.request - (event.status & 0xff_ffff)) as usize);
            if let Err(error) = self.deliver_pointer(index, pipe_index, &data) {
                log::warn!("[hid] pointer report refused: {error}");
            }
            self.queue_pointer(index, pipe_index);
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
                let next = self.devices[index].pointers[pipe_index].ring.enqueue();
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

    /// Replay one report through `deliver_pointer` on the first attached
    /// pointer interface that decodes it, as if its interrupt-IN request had
    /// completed with it. The interface's real request stays outstanding.
    /// Returns bit 0 delivered, bit 1 that interface is still armed.
    #[cfg(feature = "usb_pointer_replay")]
    pub(super) fn replay_pointer(&mut self, port: u8, data: &[u8]) -> Result<u64, &'static str> {
        let mut decodable = None;
        for (index, device) in self.devices.iter().enumerate() {
            if port != 0 && device.port != port {
                continue;
            }
            for (pipe_index, pipe) in device.pointers.iter().enumerate() {
                if let Some(layout) = pipe.layout.as_ref() {
                    if matches!(layout.decode(data), Ok(Some(_))) {
                        decodable = Some((index, pipe_index));
                        break;
                    }
                }
            }
            if decodable.is_some() {
                break;
            }
        }
        let (index, pipe_index) = decodable.ok_or("no attached pointer decodes the report")?;
        let delivered = self.deliver_pointer(index, pipe_index, data)?;
        let armed = self.devices[index].pointers[pipe_index].outstanding.is_some();
        Ok(delivered as u64 | (armed as u64) << 1)
    }
}

impl Pipe {
    pub(super) fn release(self) -> usize {
        self.ring.memory.release() + self.buffer.release()
    }
}

/// The pointer measurement's test control on a target with no host pointer
/// path (Parallels): `ioctl(fd, POINTER_REPLAY, &ReplayRequest)` hands one
/// report to the xHCI worker, which replays it through `deliver_pointer`.
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

    /// The ioctl argument: a root port (0 for any) and one report.
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct ReplayRequest {
        port: u32,
        len: u32,
        report: [u8; REPORT_MAX],
    }

    /// The pending request, taken by the worker.
    pub(in super::super) static PENDING: Mutex<Option<(u64, u8, Vec<u8>)>> = Mutex::new(None);
    static ASKED: AtomicU64 = AtomicU64::new(0);
    static DONE: AtomicU64 = AtomicU64::new(0);
    /// The last answer: bits 0-1 the worker's result, bit 2 refused.
    static ANSWER: AtomicU64 = AtomicU64::new(0);

    pub fn ioctl(arg: u64) -> SyscallResult {
        let asked = match userptr::copy_from_user(arg as *const ReplayRequest) {
            Ok(asked) => asked,
            Err(error) => return SyscallResult::Err(error),
        };
        if asked.len == 0 || asked.len as usize > REPORT_MAX || asked.port > 255 {
            return SyscallResult::Err(errno::EINVAL as u64);
        }
        let seq = ASKED.fetch_add(1, Ordering::AcqRel) + 1;
        *PENDING.lock() = Some((seq, asked.port as u8, asked.report[..asked.len as usize].to_vec()));
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
