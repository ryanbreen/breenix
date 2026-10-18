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
    errors: u8,
}

impl Controller {
    pub(super) fn attach_pointers(&mut self, index: usize) -> Result<(), &'static str> {
        let mut interfaces = Vec::new();
        crate::drivers::hid_boot::interface::pointers(&self.devices[index].config, &mut |p| {
            interfaces.push(p)
        })?;
        for interface in interfaces {
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
        let ring = Ring::new(self.address64)?;
        let buffer = Dma::new(4096, self.address64)?;
        let interval = spec::interrupt_interval(self.devices[index].speed, hid.interval)?;
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
            Err(error) => return Err(error),
        }
        self.devices[index].pointers.push(Pipe {
            dci,
            endpoint: hid.endpoint,
            ring,
            buffer,
            request: hid.max_packet as u32,
            outstanding: None,
            layout: None,
            errors: 0,
        });
        if self.devices[index].keyboard.is_none()
            && self.devices[index].pointers.len() == 1
            && self.control(index, spec::set_configuration(hid.configuration), None)? != 1
        {
            return Err("pointer SET_CONFIGURATION refused");
        }
        let mut device = self.devices.remove(index);
        // USB 2.0 9.4.3 and HID 1.11 7.1: standard interface GET_DESCRIPTOR.
        let setup = 0x0681
            | (0x22u64 << 24)
            | ((hid.interface as u64) << 32)
            | ((hid.report_length as u64) << 48);
        let descriptor = self.read_descriptor(&mut device, setup, hid.report_length);
        self.devices.insert(index, device);
        let layout = Layout::parse(&descriptor?)?;
        let pipe_index = self.devices[index].pointers.len() - 1;
        self.devices[index].pointers[pipe_index].layout = Some(layout);
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
            let data = pipe
                .buffer
                .copy(0, (pipe.request - (event.status & 0xff_ffff)) as usize);
            let decoded = pipe
                .layout
                .as_ref()
                .ok_or("pointer has no report layout")
                .and_then(|layout| layout.decode(&data));
            match decoded {
                Ok(Some(motion)) => {
                    #[cfg(target_arch = "aarch64")]
                    crate::drivers::hid_boot::state::report(self.devices[index].port, motion);
                    #[cfg(target_arch = "x86_64")]
                    let _ = motion;
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
