//! Opt-in, per-boot recovery exercise. This is never milestone evidence.
//! USB 2.0 9.4.7: an unknown configuration causes Request Error. USB 2.0
//! 9.4.3: a device descriptor has 18 bytes, even if wLength requests more.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode { ResetDevice, ShortRead }
impl Mode {
    pub(super) fn parse(bytes: &[u8]) -> Option<Self> {
        match bytes {
            b"reset-device\n" => Some(Self::ResetDevice),
            b"short-read\n" => Some(Self::ShortRead),
            _ => None,
        }
    }
    pub(super) fn name(self) -> &'static str {
        match self { Self::ResetDevice => "reset-device", Self::ShortRead => "short-read" }
    }
}

pub(super) struct Injection {
    mode: Option<Mode>,
    port: u8,
    pub(super) refusals: u32,
    pub(super) shorts: u32,
}
impl Injection {
    pub(super) const fn new() -> Self {
        Self { mode: None, port: 0, refusals: 0, shorts: 0 }
    }
    pub(super) fn arm(&mut self, mode: Mode, port: u8) {
        self.mode = Some(mode);
        self.port = port;
    }
    pub(super) fn refuse_configuration(&mut self, port: u8) -> bool {
        if self.mode == Some(Mode::ResetDevice) && self.port == port && self.refusals < 2 {
            self.refusals += 1;
            true
        } else { false }
    }
    pub(super) fn short_descriptor(&mut self, port: u8, kind: u8, len: u16) -> bool {
        if self.mode == Some(Mode::ShortRead) && self.port == port && kind == 1 && len == 18 && self.shorts == 0 {
            self.shorts += 1;
            true
        } else { false }
    }
}

#[cfg(not(test))]
pub(super) fn requested() -> Result<Option<Mode>, &'static str> {
    let guard = crate::fs::ext2::root_fs_read()?;
    let Some(fs) = guard.as_ref() else { return Ok(None); };
    let Ok(number) = fs.resolve_path("/etc/breenix/xhci-fault") else { return Ok(None); };
    let inode = fs.read_inode(number)?;
    if inode.is_dir() || inode.size() > 32 { return Err("invalid xHCI fault request"); }
    let bytes = fs.read_file_content_coherent(number, &inode)?;
    if bytes.is_empty() { return Ok(None); }
    Mode::parse(&bytes).map(Some).ok_or("invalid xHCI fault request")
}

#[cfg(not(test))]
impl super::Controller {
    pub(super) fn recovery_exercise(&mut self, mode: Mode) -> Result<(), &'static str> {
        use super::{delay, now, SECOND};
        use crate::drivers::hid_boot::interface;
        let index = self.devices.iter().position(|d| d.keyboard.is_some())
            .ok_or("fault test needs a boot keyboard")?;
        let port = self.devices[index].port;
        self.fault.arm(mode, port);
        log::info!("[xhci-fault] {} BEGIN", mode.name());
        if mode == Mode::ResetDevice {
            self.teardown(index, "fault-test reattach");
            self.attach(port, 0);
            if self.fault.refusals != 2 { return Err("configuration faults did not run"); }
        }
        let index = self.devices.iter().position(|d| d.port == port && d.keyboard.is_some())
            .ok_or("fault-test device did not re-enumerate")?;
        let interface = self.devices[index].keyboard.as_ref().ok_or("no keyboard")?.interface;
        // HID 1.11 7.2.4: duration 1 requests unchanged reports each 4 ms.
        // These are real interrupt-IN transfers, not synthetic key events.
        let periodic = interface::set_idle_indefinite(interface) | (1u64 << 24);
        if self.control(index, periodic, None)? != 1 { return Err("periodic HID reports refused"); }
        self.dispatch();
        let index = self.devices.iter().position(|d| d.port == port && d.keyboard.is_some())
            .ok_or("keyboard left during fault test")?;
        let before = self.devices[index].keyboard.as_ref().ok_or("no keyboard")?.reports;
        let retries = self.devices[index].ep0_retries;
        if mode == Mode::ShortRead {
            let mut device = self.devices.remove(index);
            let result = self.descriptor(&mut device, 1, 0, 18);
            self.devices.insert(index, device);
            let descriptor = result?;
            if descriptor.len() != 18 || descriptor[..2] != [18, 1]
                || self.devices[index].ep0_retries != retries + 1 || self.fault.shorts != 1 {
                return Err("short descriptor was not resent completely");
            }
            // A real interrupt-IN completion must have survived EP0's waits.
            self.service()?;
            let device = &self.devices[index];
            let pipe = device.keyboard.as_ref().ok_or("no keyboard")?;
            if !self.completions.iter().any(|e| e.kind() == 32 && e.slot() == device.slot
                && (e.control >> 16) & 31 == pipe.dci as u32
                && Some(e.parameter & !15) == pipe.outstanding) {
                return Err("no HID completion retained across short read");
            }
        }
        let deadline = now().saturating_add(SECOND);
        loop {
            self.service()?;
            self.dispatch();
            let pipe = self.devices.iter().find(|d| d.port == port).and_then(|d| d.keyboard.as_ref())
                .ok_or("keyboard lost after recovery")?;
            if pipe.reports > before && pipe.outstanding.is_some() { break; }
            if now() >= deadline { return Err("no input report after recovery"); }
            delay(1_000_000);
        }
        let index = self.devices.iter().position(|d| d.port == port).ok_or("keyboard left")?;
        if self.control(index, interface::set_idle_indefinite(interface), None)? != 1 {
            return Err("could not restore change-only reports");
        }
        log::info!("[xhci-fault] {} PASS: refusals={} shorts={} input-report-decoded-and-requeued",
                   mode.name(), self.fault.refusals, self.fault.shorts);
        // Keep the marker distinct from every production boot stage.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn faults_are_opt_in_bounded_and_target_one_port() {
        let mut f = Injection::new();
        assert!(!f.refuse_configuration(1));
        assert!(!f.short_descriptor(1, 1, 18));
        f.arm(Mode::ResetDevice, 2);
        assert!(!f.refuse_configuration(1));
        assert!(f.refuse_configuration(2));
        assert!(f.refuse_configuration(2));
        assert!(!f.refuse_configuration(2));
        assert_eq!(f.refusals, 2);
        let mut f = Injection::new();
        f.arm(Mode::ShortRead, 2);
        for (port, kind, len) in [(1, 1, 18), (2, 2, 18), (2, 1, 8)] {
            assert!(!f.short_descriptor(port, kind, len));
        }
        assert!(f.short_descriptor(2, 1, 18));
        assert!(!f.short_descriptor(2, 1, 18));
        assert_eq!(f.shorts, 1);
        assert_eq!(Mode::parse(b"reset-device\n").unwrap().name(), "reset-device");
        assert_eq!(Mode::parse(b"short-read\n").unwrap().name(), "short-read");
        assert_eq!(Mode::parse(b"short-read\nextra"), None);
    }
}
