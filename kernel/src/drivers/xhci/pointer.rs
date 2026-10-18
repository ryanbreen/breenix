//! HID pointer interrupt-IN pipes, configured from published USB/HID/xHCI formats.
use super::{spec, Controller, Dma, Ring, Trb};
use crate::drivers::hid_boot::{interface::PointerInterface, pointer::Layout};
use alloc::vec::Vec;

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
        let layout = match Layout::parse(&descriptor?) {
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
        log::info!("[usb] HID pointer attached: port={port} slot={slot} interface={} endpoint={:#x} report-descriptor={} bytes", hid.interface, hid.endpoint, hid.report_length);
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
            let decoded = pipe
                .layout
                .as_ref()
                .ok_or("pointer has no report layout")
                .and_then(|layout| layout.decode(&data));
            match decoded {
                Ok(Some(mut motion)) => {
                    if let Some(buttons) = motion.buttons {
                        self.devices[index].pointers[pipe_index].buttons = buttons as u8;
                        motion.buttons = Some(
                            self.devices[index]
                                .pointers
                                .iter()
                                .fold(0u32, |held, pipe| held | pipe.buttons as u32),
                        );
                    }
                    crate::drivers::hid_boot::state::report(self.devices[index].port, motion);
                }
                Ok(None) => {}
                Err(error) => log::warn!("[hid] pointer report refused: {error}"),
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
}

impl Pipe {
    pub(super) fn release(self) -> usize {
        self.ring.memory.release() + self.buffer.release()
    }
}
