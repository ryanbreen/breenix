//! Session input state and the shared /dev/uinput backend.
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawInput { pub kind: u16, pub code: u16, pub value: i32 }
impl RawInput {
    pub fn validate(self) -> Result<(), String> {
        let valid = match self.kind {
            1 => ((1..=255).contains(&self.code) || (272..=274).contains(&self.code)) && matches!(self.value, 0 | 1),
            2 => matches!(self.code, 0 | 1 | 6 | 8),
            _ => false,
        };
        if valid { Ok(()) } else { Err("invalid raw session input".into()) }
    }
}

/// A write updates held state only after the backend accepted the complete batch.
#[derive(Default)]
pub struct InputState { owner: Option<u64>, held: BTreeSet<u16> }
impl InputState {
    pub fn pause(&mut self) { self.owner = None; }
    pub fn owner(&self) -> Option<u64> { self.owner }
    pub fn regrab(&mut self, owner: u64, grab: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
        self.pause();
        if owner <= 1 { return Err("invalid input owner".into()); }
        grab()?;
        self.owner = Some(owner);
        Ok(())
    }
    pub fn send(&mut self, event: RawInput, observed: Option<u64>, write: impl FnOnce(RawInput) -> Result<(), String>) -> Result<bool, String> {
        event.validate()?;
        if self.owner.is_none() || self.owner != observed { self.pause(); return Ok(false); }
        write(event)?;
        if event.kind == 1 {
            if event.value == 1 { self.held.insert(event.code); } else { self.held.remove(&event.code); }
        }
        Ok(true)
    }
    /// Continue past a failed release, retaining that edge for an explicit retry.
    pub fn release_all(&mut self, mut write: impl FnMut(RawInput) -> Result<(), String>) -> Result<(), String> {
        self.pause();
        let mut error = None;
        for code in self.held.iter().copied().collect::<Vec<_>>() {
            match write(RawInput { kind: 1, code, value: 0 }) {
                Ok(()) => { self.held.remove(&code); }
                Err(e) => if error.is_none() { error = Some(e); },
            }
        }
        error.map_or(Ok(()), Err)
    }
}

#[cfg(target_os = "breenix")]
pub(crate) mod runtime {
    use super::{InputState, RawInput};
    use libbreenix::{error::Error, errno::Errno, input::{self, axis, key, kind, Batch, Capability, Injector, Reader}, window};

    pub(crate) struct Devices {
        keyboard: Injector,
        pointer: Injector,
        reader: Option<Reader>,
        state: InputState,
    }
    fn write(keyboard: &Injector, pointer: &Injector, event: RawInput) -> Result<(), String> {
        let device = if event.kind == kind::REL || event.code >= key::BUTTON_LEFT { pointer } else { keyboard };
        let mut storage = [0; 64];
        let mut batch = Batch::new(&mut storage);
        // Injector time is frozen at the session's UI time in step 22.6.
        let stamp = libbreenix::ui_clock::now_ns();
        batch.event(event.kind, event.code, event.value, stamp).and_then(|b| b.finish(stamp)).map_err(|e| e.to_string())?;
        device.inject(&batch).map_err(|e| e.to_string())
    }
    pub(crate) fn status() -> Result<Option<window::abi::CompositorStatus>, String> {
        let mut status = window::abi::CompositorStatus::default();
        let ret = unsafe { window::raw_call(window::abi::op::COMPOSITOR_STATUS,
            &mut status as *mut _ as *mut u8, core::mem::size_of_val(&status)) };
        if ret == -(Errno::ENOENT as i64) { return Ok(None); }
        if ret != 0 { return Err(format!("input compositor status {ret}")); }
        Ok(Some(status))
    }
    impl Devices {
        pub(crate) fn create() -> Result<Self, String> {
            let keyboard: Vec<_> = (1..=255).map(|code| Capability { kind: kind::KEY, code, minimum: 0, maximum: 1 })
                .chain([Capability { kind: kind::TEXT, code: 0, minimum: 1, maximum: 256 }]).collect();
            let pointer: Vec<_> = [key::BUTTON_LEFT, key::BUTTON_RIGHT, key::BUTTON_MIDDLE].into_iter()
                .map(|code| Capability { kind: kind::KEY, code, minimum: 0, maximum: 1 })
                .chain([axis::X, axis::Y, axis::WHEEL, axis::HWHEEL].into_iter()
                    .map(|code| Capability { kind: kind::REL, code, minimum: i32::MIN, maximum: i32::MAX })).collect();
            let keyboard = Injector::create(&keyboard, true).map_err(|e| e.to_string())?;
            let pointer = Injector::create(&pointer, true).map_err(|e| e.to_string())?;
            Ok(Self { keyboard, pointer, reader: None, state: InputState::default() })
        }
        pub(crate) fn ids(&self) -> [u32; 2] { [self.keyboard.id(), self.pointer.id()] }
        pub(crate) fn environment(&self) -> Vec<u8> {
            format!("BREENIX_INPUT_DEVICES={},{}", self.keyboard.id(), self.pointer.id()).into_bytes()
        }
        pub(crate) fn recover(&mut self, supervisor: u64) -> Result<(), String> {
            if self.reader.is_some() && self.state.owner() == Some(supervisor) { return Ok(()); }
            let reader = Reader::open(true).map_err(|e| e.to_string())?;
            let ids = self.ids();
            self.state.regrab(supervisor, || reader.grab(&ids).map_err(|e| e.to_string()))?;
            self.reader = Some(reader);
            Ok(())
        }
        /// Compositors take the grab before kiosk's ready line or bwm's first
        /// composite. Only the supervisor's successful GRAB witnesses recovery.
        pub(crate) fn observe(&mut self, compositor: Option<u64>, kiosk_ready: bool) -> Result<(), String> {
            let status = status()?;
            let owner = status.as_ref().map(|s| u64::from(s.owner));
            if self.state.owner().is_some() && self.state.owner() != owner { self.state.pause(); }
            if self.reader.is_none() && self.state.owner().is_none() {
                if let Some(s) = status.filter(|s| Some(u64::from(s.owner)) == compositor && (kiosk_ready || s.composites != 0)) {
                    self.state.regrab(u64::from(s.owner), || Ok(()))?;
                }
            }
            Ok(())
        }
        pub(crate) fn inject(&mut self, event: RawInput) -> Result<bool, String> {
            let owner = status()?.map(|s| u64::from(s.owner));
            self.state.send(event, owner, |event| write(&self.keyboard, &self.pointer, event))
        }
        pub(crate) fn text(&mut self, text: &str) -> Result<bool, String> {
            if text.is_empty() || text.len() > 256 { return Err("invalid session text length".into()); }
            let owner = status()?.map(|s| u64::from(s.owner));
            if self.state.owner().is_none() || self.state.owner() != owner {
                self.state.pause(); return Ok(false);
            }
            let mut bytes = vec![0; 64 + text.len()];
            let mut batch = Batch::new(&mut bytes);
            let stamp = libbreenix::ui_clock::now_ns();
            batch.text(text, stamp).and_then(|b| b.finish(stamp)).map_err(|e| e.to_string())?;
            self.keyboard.inject(&batch).map_err(|e| e.to_string())?;
            Ok(true)
        }
        pub(crate) fn motion(&mut self,x:i32,y:i32)->Result<bool,String> {
            let owner=status()?.map(|s|u64::from(s.owner));
            let pointer=&self.pointer;
            self.state.send(RawInput {kind:kind::REL,code:axis::X,value:x},owner,|_| {
                let mut bytes=[0;96];let mut batch=Batch::new(&mut bytes);
                let stamp=libbreenix::ui_clock::now_ns();
                batch.event(kind::REL,axis::X,x,stamp).and_then(|b|b.event(kind::REL,axis::Y,y,stamp))
                    .and_then(|b|b.finish(stamp)).map_err(|e|e.to_string())?;
                pointer.inject(&batch).map_err(|e|e.to_string())
            })
        }
        /// The recovered compositor consumes input without routing to a client.
        pub(crate) fn drain(&self) -> Result<(), String> {
            let Some(reader) = &self.reader else { return Ok(()); };
            let mut buffer = [0; 32 * 1024];
            // Bound one service turn so a live input source cannot starve control.
            for _ in 0..4 {
                let records = match reader.read(&mut buffer) {
                    Ok(records) => records,
                    Err(Error::Os(Errno::EAGAIN)) => break,
                    Err(Error::Os(Errno::EINTR)) => continue,
                    Err(e) => return Err(e.to_string()),
                };
                for record in records {
                    let event = record.map_err(|e| e.to_string())?.event;
                    reader.dispose(&input::Disposition { serial: event.serial,
                        disposition: input::disposition::WM_HANDLED, ..Default::default() }).map_err(|e| e.to_string())?;
                }
            }
            Ok(())
        }
        pub(crate) fn destroy(mut self) -> Result<(), String> {
            let result = self.state.release_all(|event| write(&self.keyboard, &self.pointer, event));
            let drained = self.drain();
            // These descriptors close before the recovered grab is dropped.
            drop(self.keyboard);
            drop(self.pointer);
            drop(self.reader);
            result.and(drained)
        }
    }
}
