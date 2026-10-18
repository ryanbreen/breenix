//! HID boot keyboards on the x86-metal xHCI: configuration, polling,
//! hot-plug, and reclaiming a detached device's slot, rings and memory.
//!
//! Source authority: xHCI 1.2 4.3.5 and 4.6.6 (Configure Endpoint, then
//! SET_CONFIGURATION), 6.2.2.2, 6.2.3.2 and 6.2.3.6 (input contexts and the
//! interval), 4.6.4 and 4.6.9 (Stop Endpoint, then Disable Slot, then free
//! the device's structures), 4.6.8, 4.6.10 and 4.10.2.1 (halt recovery),
//! 4.19 and 5.4.8 (port changes and port power); USB 2.0 9.4; HID 1.11 7.2
//! and Appendices B and F. A refused configuration is retried on a reset
//! device (4.10.2.1.2). Devices behind hubs are not driven: the machine's
//! port census found its keyboard on a root port.
use super::recovery::{self, Attach, DEVICE_RESETS};
use super::{
    delay, now, spec, wr, Controller, Device, Dma, Ring, Trb, CONSOLE_READY, CONSOLE_START, LEDS,
    PORT_CHANGES, REPLUG, SECOND, STATE, STATE_READY,
};
use crate::drivers::hid_boot::{interface, report};
use alloc::vec::Vec;
use core::sync::atomic::{fence, Ordering};

/// Interrupt-IN errors in a row after which the keyboard is torn down.
const ERRORS_BEFORE_TEARDOWN: u32 = 3;
/// Detach and reattach rounds in a console boot's replug exercise.
const REPLUG_ROUNDS: u32 = 10;
const CCS: u32 = 1;
const PED: u32 = 1 << 1;
const PP: u32 = 1 << 9;
const CSC: u32 = 1 << 17;

/// A configured boot keyboard's interrupt-IN pipe and key state.
pub(super) struct Pipe {
    pub(super) interface: u8,
    endpoint: u8,
    pub(super) dci: u8,
    ring: Ring,
    buffer: Dma,
    request: u32,
    pub(super) outstanding: Option<u64>,
    state: report::Keyboard,
    errors: u32,
    bad_reports: u64,
    pub(super) reports: u64,
}

impl Controller {
    /// Configure the keyboards the census enumerated, then serve the
    /// controller: events, port changes, typematic and console delivery.
    pub(super) fn serve(&mut self) -> Result<(), &'static str> {
        // A refused configuration replaces its device, so go by port.
        let ports: Vec<u8> = self.devices.iter().map(|d| d.port).collect();
        for port in ports {
            if let Some(index) = self.devices.iter().position(|d| d.port == port) {
                let first = self.attach_keyboard(index, 0);
                recovery::reset_while_refused(&mut Reattach { controller: self, port }, first);
                #[cfg(target_arch = "aarch64")]
                if let Some(index) = self.devices.iter().position(|d| d.port == port) {
                    if let Err(error) = self.attach_pointers(index) {
                        log::error!("[usb] port {port}: pointer attach FAILED: {error}");
                    }
                }
            }
        }
        STATE.store(STATE_READY, Ordering::Release);
        loop {
            let deadline = self.next_deadline();
            if self.wait_work(deadline, true)? {
                self.service()?;
            }
            self.dispatch();
            self.tick_keyboards();
            if CONSOLE_START.swap(false, Ordering::SeqCst) {
                // A fault-test kernel's recovery exercise replaces the replug exercise.
                #[cfg(feature = "xhci_fault_inject")]
                let exercised = self.fault_exercise()?;
                #[cfg(not(feature = "xhci_fault_inject"))]
                let exercised = false;
                if REPLUG.swap(false, Ordering::Relaxed) && !exercised {
                    self.replug_exercise();
                }
                self.console.arm();
                CONSOLE_READY.complete(1);
            }
            if LEDS.swap(false, Ordering::SeqCst) {
                self.sync_leds();
            }
            #[cfg(all(target_arch = "aarch64", feature = "usb_pointer_replay"))]
            if super::REPLAY.swap(false, Ordering::SeqCst) {
                let pending = super::pointer::replay::PENDING.lock().take();
                if let Some((seq, port, endpoint, report)) = pending {
                    let result = self.replay_pointer(port, endpoint, &report);
                    super::pointer::replay::answer(seq, result);
                }
            }
        }
    }

    /// Send attached keyboards the console's absolute lock state when it
    /// differs from what they were last sent.
    fn sync_leds(&mut self) {
        let leds = self.console.leds();
        if leds != self.keyboard_leds {
            self.keyboard_leds = leds;
            for keyboard in 0..self.devices.len() {
                if self.devices[keyboard].keyboard.is_some() {
                    self.send_leds(keyboard);
                }
            }
        }
    }

    /// Now, when events are already waiting; else the next repeat.
    fn next_deadline(&self) -> Option<u64> {
        if !self.completions.is_empty() || !self.port_events.is_empty() {
            return Some(now());
        }
        self.devices
            .iter()
            .filter_map(|d| d.keyboard.as_ref()?.state.repeat_due())
            .min()
    }

    pub(super) fn dispatch(&mut self) {
        while let Some(port) = self.port_events.pop_front() {
            self.port_changed(port);
        }
        let events: Vec<Trb> = self.completions.drain(..).collect();
        for event in events {
            // Commands and control transfers wait for their own completions;
            // anything else here is a keyboard's or a stale event.
            if event.kind() != 32 {
                continue;
            }
            #[cfg(target_arch = "aarch64")]
            if self.dispatch_pointer(event) {
                continue;
            }
            let dci = ((event.control >> 16) & 31) as u8;
            let pointer = event.parameter & !15;
            if let Some(index) = self.devices.iter().position(|d| {
                d.slot == event.slot()
                    && d.keyboard
                        .as_ref()
                        .is_some_and(|p| p.dci == dci && p.outstanding == Some(pointer))
            }) {
                self.keyboard_event(index, event);
            }
        }
    }

    /// Configure an enumerated device's boot keyboard, if it has one. A
    /// device that still refuses after EP0 recovery has a control endpoint
    /// the next SETUP did not clear, so its slot is given back and the
    /// result is `Refused`: the caller resets and enumerates it again
    /// (4.10.2.1.2, `recovery::reset_while_refused`). `resets` counts the
    /// resets this attach has used.
    fn attach_keyboard(&mut self, index: usize, resets: u32) -> Attach {
        let port = self.devices[index].port;
        let boot = match interface::find(&self.devices[index].config) {
            Ok(Some(boot)) => boot,
            Ok(None) => return Attach::Settled,
            Err(error) => {
                log::error!("[usb] port {port}: configuration descriptor unusable: {error}");
                return Attach::Settled;
            }
        };
        let Err(error) = self.configure_keyboard(index, boot, resets) else {
            return Attach::Settled;
        };
        log::error!("[usb] port {port}: boot keyboard configuration FAILED: {error}");
        // Give the slot back; the device is not driven in this state.
        let device = self.devices.remove(index);
        let slot = device.slot;
        match self.reclaim(device) {
            Some(pages) => {
                log::info!("[usb] port {port}: unconfigured keyboard's slot {slot} disabled, {pages} pages returned");
                Attach::Refused
            }
            None => {
                log::error!("[usb] port {port}: slot {slot} not reclaimed; its memory stays with the controller");
                Attach::Settled
            }
        }
    }

    fn configure_keyboard(
        &mut self,
        index: usize,
        boot: interface::BootKeyboard,
        resets: u32,
    ) -> Result<(), &'static str> {
        let (slot, port, speed) = {
            let d = &self.devices[index];
            (d.slot, d.port, d.speed)
        };
        let interval = spec::interrupt_interval(speed, boot.interval)?;
        let dci = boot.dci();
        let burst = if speed >= 480_000_000 {
            boot.extra_transactions
        } else {
            0
        };
        let ring = Ring::new(self.address64)?;
        let buffer = match Dma::new(4096, self.address64) {
            Ok(buffer) => buffer,
            Err(error) => {
                ring.memory.release();
                return Err(error);
            }
        };
        let request = (boot.max_packet as u32).max(report::REPORT_LEN as u32);
        // Input Control (A0 and the endpoint), Slot (Context Entries) and the
        // endpoint's context; the rest of the input context zeroed (4.6.6, 6.2.2.2, 6.2.3.2).
        let d = &self.devices[index];
        for offset in (0..33 * self.contexts.stride).step_by(4) {
            d.input.put32(offset, 0);
        }
        d.input.put32(4, 1 | 1 << dci);
        let slot_at = self.contexts.input(0);
        for dword in 0..3 {
            d.input.put32(
                slot_at + dword * 4,
                d.output.get32(self.contexts.output(0) + dword * 4),
            );
        }
        let dw0 = d.input.get32(slot_at);
        d.input
            .put32(slot_at, (dw0 & !(31 << 27)) | (dci as u32) << 27);
        let ep_at = self.contexts.input(dci as usize);
        for (i, value) in
            spec::interrupt_in_context(interval, boot.max_packet, burst, ring.memory.phys)
                .iter()
                .enumerate()
        {
            d.input.put32(ep_at + i * 4, *value);
        }
        fence(Ordering::Release);
        let input = d.input.phys;
        // 4.3.5: SET_CONFIGURATION only after Configure Endpoint succeeded.
        match self.command_code(spec::configure_endpoint(input, slot)) {
            Ok(1) => {}
            Ok(code) => {
                // An unsuccessful command leaves the contexts unchanged (4.6.6).
                ring.memory.release();
                buffer.release();
                log::error!("[usb] port {port}: Configure Endpoint code={code}");
                return Err("Configure Endpoint refused the interrupt-IN endpoint");
            }
            // Unanswered: the ring and buffer stay with the controller.
            Err(error) => return Err(error),
        }
        self.devices[index].keyboard = Some(Pipe {
            interface: boot.interface,
            endpoint: boot.endpoint,
            dci,
            ring,
            buffer,
            request,
            outstanding: None,
            state: report::Keyboard::new(),
            errors: 0,
            bad_reports: 0,
            reports: 0,
        });
        // USB 2.0 9.4.7, then HID 1.11 F.5: boot protocol after configuration;
        // Set_Idle(0) so reports come only on change (7.2.4, F.4).
        for (setup, what) in [
            (
                spec::set_configuration(boot.configuration),
                "SET_CONFIGURATION",
            ),
            (
                interface::set_boot_protocol(boot.interface),
                "Set_Protocol(Boot)",
            ),
            (interface::set_idle_indefinite(boot.interface), "Set_Idle"),
        ] {
            let code = self.control(index, setup, None)?;
            if code != 1 {
                log::error!("[usb] port {port}: {what} completion code={code}");
                return Err("boot keyboard request refused");
            }
        }
        // A deterministic driver fault: some keyboards accept even invalid
        // configuration values. Fail at the configuration boundary after the
        // real requests completed, then use the ordinary reclaim/reset path.
        #[cfg(feature = "xhci_fault_inject")]
        if self.fault.refuse_configuration(port) {
            log::info!("[xhci-fault] configuration refusal injected: reset-attempt={resets}");
            return Err("fault-test configuration refusal");
        }
        let leds = self.send_leds(index);
        self.queue_report(index)?;
        log::info!(
            "[usb] boot keyboard attached: port={port} path=root speed={speed} bit/s slot={slot} \
             interface={} endpoint={:#04x} max-packet={} bInterval={} interval={interval} leds={leds} \
             ep0-retries={} device-resets={resets}",
            boot.interface,
            boot.endpoint,
            boot.max_packet,
            boot.interval,
            self.devices[index].ep0_retries
        );
        Ok(())
    }

    /// A control transfer on EP0 with no data stage or one OUT byte. Returns
    /// its completion code; a request that halted EP0 is recovered and resent
    /// (`Controller::ep0_transfer`).
    pub(super) fn control(&mut self, index: usize, setup: u64, out: Option<u8>) -> Result<u8, &'static str> {
        // The device leaves the list for the transfer so the controller can
        // wait for it. The list is read by `dispatch`, between transfers.
        let mut device = self.devices.remove(index);
        let result = self.ep0_transfer(&mut device, setup, &|device: &mut Device| {
            let mut pointers = Vec::new();
            match out {
                None => pointers.push(device.ep0.push(spec::setup_no_data(setup))),
                Some(byte) => {
                    device.data.put8(0, byte);
                    pointers.push(device.ep0.push(spec::setup_data_out(setup)));
                    pointers.push(device.ep0.push(spec::data_out(device.data.phys, 1)));
                }
            }
            pointers.push(device.ep0.push(spec::status_in()));
            pointers
        });
        self.devices.insert(index, device);
        Ok(result?.code())
    }

    /// Recover a halted interrupt-IN endpoint: Reset Endpoint (TSP 0),
    /// CLEAR_FEATURE(ENDPOINT_HALT), then point its dequeue at the next free
    /// TRB, past the failed TD (4.6.8, 4.6.10, 4.10.2.1.1).
    fn reset_pipe(&mut self, index: usize, dci: u8, endpoint: u8) -> Result<(), &'static str> {
        let slot = self.devices[index].slot;
        self.reset_endpoint(slot, dci)?;
        if self.control(index, spec::clear_endpoint_halt(endpoint), None)? != 1 {
            return Err("CLEAR_FEATURE(ENDPOINT_HALT) refused");
        }
        let next = self.devices[index]
            .keyboard
            .as_ref()
            .ok_or("device has no keyboard pipe")?
            .ring
            .enqueue();
        self.set_dequeue(slot, dci, next)
    }

    /// Send the absolute LED state with Set_Report(Output) (HID 1.11 7.2.2,
    /// B.1). The request is optional for a boot keyboard, so a refusal is
    /// reported, not fatal.
    fn send_leds(&mut self, index: usize) -> &'static str {
        let Some(pipe) = self.devices[index].keyboard.as_ref() else {
            return "none";
        };
        let (setup, leds) = (
            interface::set_led_report(pipe.interface),
            self.keyboard_leds,
        );
        match self.control(index, setup, Some(leds)) {
            Ok(1) => "accepted",
            Ok(_) => "refused",
            Err(_) => "failed",
        }
    }

    /// Post one interrupt-IN TD for the next report.
    fn queue_report(&mut self, index: usize) -> Result<(), &'static str> {
        let device = &mut self.devices[index];
        let slot = device.slot;
        let pipe = device
            .keyboard
            .as_mut()
            .ok_or("device has no keyboard pipe")?;
        let pointer = pipe
            .ring
            .push(spec::normal_in(pipe.buffer.phys, pipe.request));
        pipe.outstanding = Some(pointer);
        let dci = pipe.dci;
        self.ring_doorbell(slot, dci as u32);
        Ok(())
    }

    fn keyboard_event(&mut self, index: usize, event: Trb) {
        let port = self.devices[index].port;
        let Some(pipe) = self.devices[index].keyboard.as_mut() else {
            return;
        };
        pipe.outstanding = None;
        let (dci, endpoint) = (pipe.dci, pipe.endpoint);
        match event.code() {
            // Success or Short Packet: the TD's length less the residual.
            1 | 13 => {
                pipe.errors = 0;
                let residual = event.status & 0x00ff_ffff;
                let report = if residual <= pipe.request {
                    pipe.buffer.copy(0, (pipe.request - residual) as usize)
                } else {
                    Vec::new()
                };
                let now = now();
                let Controller {
                    devices, console, ..
                } = self;
                let Some(pipe) = devices[index].keyboard.as_mut() else {
                    return;
                };
                match pipe.state.report(&report, now, &mut |code, value| console.event(port, code, value)) {
                    Ok(summary) => {
                        pipe.reports += 1;
                        console.note_report(&summary);
                    }
                    Err(report::ReportError::Length(length)) => {
                        pipe.bad_reports += 1;
                        if pipe.bad_reports == 1 {
                            log::warn!("[hid] port {port}: {length}-byte report ignored; a boot report is 8 bytes");
                        }
                    }
                }
                // Rearm before synchronous SET_REPORT: the console can ask
                // the host for another key as soon as this report is delivered.
                // Keep interrupt-IN available while the LED transfer waits.
                if let Err(error) = self.queue_report(index) {
                    log::error!("[usb] port {port}: cannot post the next report: {error}");
                }
                self.sync_leds();
            }
            // Stopped by a teardown in progress.
            26..=28 => {}
            code => {
                pipe.errors += 1;
                if pipe.errors > ERRORS_BEFORE_TEARDOWN {
                    log::error!(
                        "[usb] port {port}: interrupt-IN failed {} times, last code={code}",
                        pipe.errors
                    );
                    self.teardown(index, "interrupt-IN errors");
                    return;
                }
                let recovered = self
                    .reset_pipe(index, dci, endpoint)
                    .and_then(|()| self.queue_report(index));
                if let Err(error) = recovered {
                    log::error!("[usb] port {port}: interrupt-IN recovery after code={code} FAILED: {error}");
                    self.teardown(index, "interrupt-IN recovery failed");
                }
            }
        }
    }

    fn tick_keyboards(&mut self) {
        let now = now();
        let Controller {
            devices, console, ..
        } = self;
        for device in devices.iter_mut() {
            if let Some(pipe) = device.keyboard.as_mut() {
                let port = device.port;
                pipe.state
                    .tick(now, &mut |code, value| console.event(port, code, value));
            }
        }
    }

    /// One Port Status Change Event: acknowledge the port's change bits, tear
    /// down a device that left (or was replaced), and attach a new connection.
    fn port_changed(&mut self, port: u8) {
        if port == 0 || port > self.ports {
            return;
        }
        let status = self.portsc(port);
        self.port_action(port, status & PORT_CHANGES);
        let connected = status & CCS != 0;
        if let Some(index) = self.devices.iter().position(|d| d.port == port) {
            if connected && status & PED != 0 && status & CSC == 0 {
                return;
            }
            let why = if !connected {
                "disconnected"
            } else if status & PED == 0 {
                "port disabled"
            } else {
                "reconnected"
            };
            self.teardown(index, why);
        }
        // Only a connect change attaches: our own port resets also post
        // events, and a device whose attach used its resets is not retried.
        if connected && status & CSC != 0 {
            self.attach(port);
        }
    }

    /// Enumerate and configure the device on `port`, resetting it while
    /// it is refused and resets are left.
    pub(super) fn attach(&mut self, port: u8) {
        let first = self.attach_once(port, 0);
        recovery::reset_while_refused(&mut Reattach { controller: self, port }, first);
        #[cfg(target_arch = "aarch64")]
        if let Some(index) = self.devices.iter().position(|d| d.port == port) {
            if let Err(error) = self.attach_pointers(index) {
                log::error!("[usb] port {port}: pointer attach FAILED: {error}");
            }
        }
    }

    fn attach_once(&mut self, port: u8, resets: u32) -> Attach {
        match self.enumerate(port) {
            Ok(()) => {
                let index = self.devices.len() - 1;
                self.attach_keyboard(index, resets)
            }
            Err(error) => {
                // `enumerate` has given the slot back.
                log::error!("[usb] port {port}: attach FAILED: {error}");
                Attach::Refused
            }
        }
    }

    pub(super) fn teardown(&mut self, index: usize, why: &str) {
        let mut device = self.devices.remove(index);
        #[cfg(target_arch = "aarch64")]
        if !device.pointers.is_empty() {
            crate::input::pointer::release(device.port);
            super::pointer::detached(device.port);
        }
        if let Some(pipe) = device.keyboard.as_mut() {
            pipe.state
                .release_all(&mut |code, value| self.console.event(device.port, code, value));
        }
        let (port, slot, keyboard) = (device.port, device.slot, device.keyboard.is_some());
        let reclaimed = self.reclaim(device);
        match (keyboard, reclaimed) {
            (true, Some(pages)) => log::info!(
                "[usb] boot keyboard detached: port={port} slot={slot} {why}; endpoints stopped, \
                 slot disabled, {pages} pages returned, keys and repeat released"
            ),
            (false, Some(pages)) => log::info!(
                "[usb] port {port}: device detached ({why}); slot {slot} disabled, {pages} pages returned"
            ),
            (_, None) => log::error!(
                "[usb] port {port}: slot {slot} ({why}) not reclaimed; its memory stays with the controller"
            ),
        }
    }

    /// Stop each endpoint, take its events, disable the slot, clear its DCBAA
    /// entry and return the device's memory (4.6.4, 4.6.9). Returns the pages
    /// freed; an empty result when the controller did not give the slot back,
    /// in which case the memory stays pinned for it.
    pub(super) fn reclaim(&mut self, device: Device) -> Option<usize> {
        let slot = device.slot;
        let mut dcis = alloc::vec![1u8];
        dcis.extend(device.keyboard.as_ref().map(|p| p.dci));
        #[cfg(target_arch = "aarch64")]
        dcis.extend(device.pointers.iter().map(|p| p.dci));
        for dci in dcis {
            match self.command_code(spec::stop_endpoint(slot, dci)) {
                // Stopped, or not running (halted, stopped or not yet enabled).
                Ok(1) | Ok(19) => {}
                Ok(code) => log::warn!("[usb] slot {slot}: Stop Endpoint {dci} code={code}"),
                Err(error) => {
                    log::error!("[usb] slot {slot}: Stop Endpoint {dci}: {error}");
                    return None;
                }
            }
        }
        // Each Stopped Transfer Event precedes its command's completion (4.6.9).
        self.completions
            .retain(|e| !(e.kind() == 32 && e.slot() == slot));
        match self.command_code(spec::disable_slot(slot)) {
            Ok(1) => {}
            Ok(code) => {
                log::error!("[usb] slot {slot}: Disable Slot code={code}");
                return None;
            }
            Err(error) => {
                log::error!("[usb] slot {slot}: Disable Slot: {error}");
                return None;
            }
        }
        self.dcbaa.put64(slot as usize * 8, 0);
        let mut pages = device.output.release()
            + device.input.release()
            + device.ep0.memory.release()
            + device.data.release();
        if let Some(pipe) = device.keyboard {
            pages += pipe.ring.memory.release() + pipe.buffer.release();
        }
        #[cfg(target_arch = "aarch64")]
        for pipe in device.pointers {
            pages += pipe.release();
        }
        Some(pages)
    }

    /// Once per console boot: detach and reattach each boot keyboard
    /// `REPLUG_ROUNDS` times so the teardown and attach paths run on the
    /// machine, repeatedly. With port power control (HCCPARAMS1.PPC, 5.3.6)
    /// the port is powered off and on; a powered-off port reports no detach
    /// (5.4.8), so the teardown is taken here, and the attach follows the
    /// connection the port reports once powered (4.19.3). Without it, the
    /// port is disabled (PED written 1), torn down and attached again.
    fn replug_exercise(&mut self) {
        let ports: Vec<u8> = self
            .devices
            .iter()
            .filter(|d| d.keyboard.is_some())
            .map(|d| d.port)
            .collect();
        if ports.is_empty() {
            log::info!("[usb] replug exercise: no boot keyboard attached");
            return;
        }
        let power = self.hcc & (1 << 3) != 0;
        for port in ports {
            let mut reattached = 0;
            for round in 1..=REPLUG_ROUNDS {
                if !self
                    .devices
                    .iter()
                    .any(|d| d.port == port && d.keyboard.is_some())
                {
                    break;
                }
                self.replug(port, power, round);
                if self
                    .devices
                    .iter()
                    .any(|d| d.port == port && d.keyboard.is_some())
                {
                    reattached += 1;
                }
            }
            log::info!("[usb] replug exercise: port {port}: reattached {reattached} of {REPLUG_ROUNDS} rounds");
        }
    }

    fn replug(&mut self, port: u8, power: bool, round: u32) {
        let register = 0x400 + (port as usize - 1) * 16;
        if power {
            log::info!(
                "[usb] replug exercise: port {port}: round {round}: port power off, then on"
            );
            wr(self.op, register, spec::port_power_off(self.portsc(port)));
            // 5.4.8: confirm PP before writing it again.
            if super::until(self.op, register, PP, 0, SECOND).is_err() {
                log::error!("[usb] replug exercise: port {port}: PP did not clear");
                return;
            }
            self.port_changed(port);
            delay(100_000_000);
            self.port_action(port, PP);
            if super::until(self.op, register, PP, PP, SECOND).is_err() {
                log::error!("[usb] replug exercise: port {port}: PP did not set");
                return;
            }
            // The device connects once powered; attach it now (CSC), so the
            // next round finds it. Its queued event then reports no change.
            if super::until(self.op, register, CCS, CCS, SECOND).is_err() {
                log::error!("[usb] replug exercise: port {port}: no connection after power on");
                return;
            }
            self.port_changed(port);
        } else {
            log::info!("[usb] replug exercise: port {port}: round {round}: port disable, then reset and attach");
            self.port_action(port, PED);
            if super::until(self.op, register, PED, 0, SECOND).is_err() {
                log::error!("[usb] replug exercise: port {port}: PED did not clear");
                return;
            }
            self.port_changed(port);
            if self.portsc(port) & CCS != 0 {
                self.attach(port);
            }
        }
    }
}

/// The hardware side of `recovery::reset_while_refused` for one root port.
struct Reattach<'a> {
    controller: &'a mut Controller,
    port: u8,
}
impl recovery::Port for Reattach<'_> {
    fn connected(&self) -> bool {
        self.controller.portsc(self.port) & CCS != 0
    }
    fn reset(&mut self, reset: u32) -> Attach {
        log::warn!("[usb] port {}: resetting the device ({reset} of {DEVICE_RESETS})", self.port);
        self.controller.attach_once(self.port, reset)
    }
}
