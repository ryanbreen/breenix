//! Shared task-context input transport, reader custody and virtual devices.
//! Driver ingress adapters keep allocation outside IRQ context.
use crate::{
    ipc::object::IpcFile,
    syscall::{errno::*, userptr},
    task::waitqueue::WaitQueueHead,
};
use alloc::{
    boxed::Box,
    collections::{BTreeMap, VecDeque},
    sync::Arc,
    vec::Vec,
};
use core::sync::atomic::{AtomicU64, Ordering};
use input_abi::{axis, disposition, ioctl, key, kind, reason, syn, Capability, Disposition, Event};

const MAX_CAPABILITIES: usize = 512;
const LEDGER_EVENTS: usize = 8192;
const MAX_BATCH: usize = input_abi::READER_EVENTS - 1;

#[derive(Clone)]
struct Record {
    event: Event,
    payload: Vec<u8>,
}
struct Batch {
    records: Vec<Record>,
    bytes: Vec<u8>,
}
struct Reader {
    batches: VecDeque<Arc<Batch>>,
    events: usize,
    dropped: Option<Event>,
    wake: Arc<WaitQueueHead>,
}
struct Device {
    capabilities: Vec<Capability>,
    injector_time: bool,
    last_time: u64,
    modifiers: u8,
    reserved: u16,
    held: BTreeMap<u16, Event>,
}
struct Held {
    event: Event,
    window: u64,
}
struct Grab {
    reader: u64,
    devices: Vec<u32>,
    holder: u64,
    epoch: u64,
}
fn fold_modifiers(devices: impl Iterator<Item = (u32, u8)>, selection: Option<&[u32]>) -> u8 {
    devices
        .filter(|(id, _)| selection.is_none_or(|ids| ids.is_empty() || ids.contains(id)))
        .fold(0, |all, (_, modifiers)| all | modifiers)
}
fn publication_flags(modifiers: u8, origin: u32) -> u32 {
    u32::from(modifiers & input_abi::modifier::MASK as u8) | (origin & input_abi::SYNTHETIC)
}
struct Row {
    event: Event,
    reader: Option<u64>,
    outcome: Option<Disposition>,
}
struct Release {
    window: u64,
    source: Event,
}
struct ConsoleKey {
    code: u16,
    value: i32,
    modifiers: super::event::Modifiers,
    reserved: bool,
}
struct State {
    next_reader: u64,
    next_device: u32,
    next_hardware: u32,
    next_serial: u64,
    readers: BTreeMap<u64, Reader>,
    devices: BTreeMap<u32, Device>,
    ledger: VecDeque<Row>,
    grab: Option<Grab>,
    releases: VecDeque<Release>,
    recipients: BTreeMap<(u32, u64), Held>,
    removals: VecDeque<u32>,
    next_derived: u64,
    derivations: BTreeMap<(u64, u32), Disposition>,
}
impl State {
    const fn new() -> Self {
        Self {
            next_reader: 1,
            next_device: 1024,
            next_hardware: 1,
            next_serial: 1,
            readers: BTreeMap::new(),
            devices: BTreeMap::new(),
            ledger: VecDeque::new(),
            grab: None,
            releases: VecDeque::new(),
            recipients: BTreeMap::new(),
            removals: VecDeque::new(),
            next_derived: 1 << 63,
            derivations: BTreeMap::new(),
        }
    }
    fn finish(&mut self, serial: u64, outcome: u32, why: u32) {
        if let Some(row) = row_mut(&mut self.ledger, serial) {
            if row.outcome.is_none() {
                row.outcome = Some(Disposition {
                    serial,
                    disposition: outcome,
                    reason: why,
                    ..Disposition::default()
                });
            }
        }
    }
    fn release_grab(&mut self, reader: u64, outcome: u32, why: u32) {
        if self.grab.as_ref().is_none_or(|g| g.reader != reader) {
            return;
        }
        let selection = self.grab.take().unwrap().devices;
        for row in self
            .ledger
            .iter_mut()
            .filter(|row| row.reader == Some(reader))
        {
            if row.outcome.is_none() {
                row.outcome = Some(Disposition {
                    serial: row.event.serial,
                    disposition: outcome,
                    reason: why,
                    ..Disposition::default()
                });
            }
        }
        // Retain recipient routing before clearing held state. A task-context
        // service delivers derived releases outside descriptor teardown.
        let keys: Vec<_> = self
            .recipients
            .keys()
            .copied()
            .filter(|(id, _)| selection.is_empty() || selection.contains(id))
            .collect();
        for key in keys {
            let held = self.recipients.remove(&key).unwrap();
            self.releases.push_back(Release {
                window: held.window,
                source: held.event,
            });
        }
        for (id, d) in &mut self.devices {
            if selection.is_empty() || selection.contains(id) {
                d.modifiers = 0;
                d.reserved = 0;
                d.held.clear();
            }
        }
    }
    fn removal_count(&self, id: u32) -> usize {
        self.devices.get(&id).map_or(0, |d| d.held.len() + 1)
    }
    fn removal_ready(&self) -> bool {
        self.removals.front().is_some_and(|id| {
            self.ledger.len() + self.removal_count(*id)
                <= LEDGER_EVENTS
                    + self
                        .ledger
                        .iter()
                        .take_while(|row| row.outcome.is_some())
                        .count()
        })
    }
    fn remove_device(&mut self, id: u32) -> Vec<ConsoleKey> {
        let d = self.devices.get(&id).unwrap();
        let time_ns = if d.injector_time {
            d.last_time
        } else {
            crate::window::events::now_ns()
        };
        let mut records: Vec<_> = d
            .held
            .values()
            .map(|press| Record {
                event: Event {
                    kind: kind::KEY,
                    code: press.code,
                    value: 0,
                    time_ns,
                    ..Event::default()
                },
                payload: Vec::new(),
            })
            .collect();
        records.push(Record {
            event: Event {
                kind: kind::SYN,
                code: syn::REPORT,
                time_ns,
                ..Event::default()
            },
            payload: Vec::new(),
        });
        let console = self.publish(id, records);
        self.devices.remove(&id);
        console
    }

    fn reserve_ledger(&mut self, count: usize) -> Result<(), u64> {
        if self
            .next_serial
            .checked_add(count as u64)
            .is_none_or(|end| end >= 1 << 63)
        {
            return Err(ENOSPC as u64);
        }
        while self.ledger.len() + count > LEDGER_EVENTS {
            let Some(row) = self.ledger.front() else {
                break;
            };
            if row.outcome.is_none() {
                let event = row.event;
                let reader = row.reader;
                // Custody cannot pin the bounded history indefinitely. Report
                // the loss before retiring this row, just as reader overflow does.
                self.finish(event.serial, disposition::LOST, reason::OVERFLOW);
                if let Some(reader) = reader.and_then(|id| self.readers.get_mut(&id)) {
                    reader.dropped = Some(Event {
                        kind: kind::SYN,
                        code: syn::DROPPED,
                        ..event
                    });
                }
            }
            self.ledger.pop_front();
        }
        Ok(())
    }
    fn publish(&mut self, device: u32, mut records: Vec<Record>) -> Vec<ConsoleKey> {
        let mut console = Vec::new();
        let holder = self.grab.as_ref().map(|g| g.reader);
        let selected = self
            .grab
            .as_ref()
            .is_none_or(|g| g.devices.is_empty() || g.devices.contains(&device));
        let mut reserved = Vec::with_capacity(records.len());
        for record in &mut records {
            let e = &mut record.event;
            e.device = device;
            e.serial = self.next_serial;
            self.next_serial += 1;
            let d = self.devices.get_mut(&device).expect("live input device");
            if e.kind == kind::SYN && e.code == syn::DROPPED {
                d.held.clear();
                d.modifiers = 0;
                d.reserved = 0;
            }
            if e.kind == kind::KEY {
                d.modifiers = super::event::update_modifiers(d.modifiers, e.code, e.value);
                if e.value == 1 {
                    d.held.insert(e.code, *e);
                }
                if e.value == 0 {
                    d.held.remove(&e.code);
                }
            }
            let mods = super::event::Modifiers(fold_modifiers(
                self.devices.iter().map(|(&id, d)| (id, d.modifiers)),
                self.grab.as_ref().map(|g| g.devices.as_slice()),
            ));
            let flags = mods.shift() as u8
                | (mods.ctrl() as u8) << 1
                | (mods.alt() as u8) << 2
                | (mods.meta() as u8) << 3;
            let trace_flags = publication_flags(flags, e.flags);
            e.flags = u32::from(flags);
            let d = self.devices.get_mut(&device).unwrap();
            let console_reserved = e.kind == kind::KEY
                && super::event::reserved(&mut d.reserved, e.code, e.value, mods);
            reserved.push(console_reserved);
            if e.kind == kind::KEY && (holder.is_none() || console_reserved) && selected {
                console.push(ConsoleKey {
                    code: e.code,
                    value: e.value,
                    modifiers: mods,
                    reserved: console_reserved,
                });
            }
            let outcome = if e.kind == kind::SYN && e.code == syn::DROPPED {
                Some(Disposition {
                    serial: e.serial,
                    disposition: disposition::LOST,
                    reason: reason::OVERFLOW,
                    ..Disposition::default()
                })
            } else if !selected {
                Some(Disposition {
                    serial: e.serial,
                    disposition: disposition::REJECTED,
                    reason: reason::NOT_SELECTED,
                    ..Disposition::default()
                })
            } else if holder.is_none() || console_reserved
            {
                Some(Disposition {
                    serial: e.serial,
                    disposition: disposition::WM_HANDLED,
                    ..Disposition::default()
                })
            } else {
                None
            };
            crate::window::pipeline::record(1, device as u64, e.time_ns, e.serial);
            crate::window::pipeline::record(16, device as u64, u64::from(trace_flags), e.serial);
            self.ledger.push_back(Row {
                event: *e,
                reader: holder,
                outcome,
            });
        }
        let all = Arc::new(batch(records));
        let grabbed = if holder.is_some() {
            Some(Arc::new(batch(
                all.records
                    .iter()
                    .zip(&reserved)
                    .filter(|(_, reserved)| !**reserved)
                    .map(|(r, _)| r.clone())
                    .collect(),
            )))
        } else {
            None
        };
        let mut lost = Vec::new();
        for (&id, reader) in &mut self.readers {
            if holder == Some(id) && !selected {
                continue;
            }
            let b = if holder == Some(id) {
                grabbed.as_ref().unwrap()
            } else {
                &all
            };
            if b.records.is_empty() {
                continue;
            }
            while reader.events + b.records.len() + usize::from(reader.dropped.is_some())
                > input_abi::READER_EVENTS
            {
                let Some(old) = reader.batches.pop_front() else {
                    break;
                };
                reader.events -= old.records.len();
                for r in &old.records {
                    if holder == Some(id) {
                        lost.push(r.event.serial);
                    }
                }
                reader.dropped = Some(Event {
                    kind: kind::SYN,
                    code: syn::DROPPED,
                    device,
                    serial: old.records.last().unwrap().event.serial,
                    time_ns: old.records.last().unwrap().event.time_ns,
                    ..Event::default()
                });
            }
            if reader.events + b.records.len() + usize::from(reader.dropped.is_some())
                > input_abi::READER_EVENTS
            {
                // A concurrent read owns the remaining records; retain its lease
                // and report this whole new batch as lost instead of exceeding bounds.
                if holder == Some(id) {
                    lost.extend(b.records.iter().map(|r| r.event.serial));
                }
                let e = b.records.last().unwrap().event;
                reader.dropped = Some(Event {
                    kind: kind::SYN,
                    code: syn::DROPPED,
                    ..e
                });
                continue;
            }
            reader.events += b.records.len();
            reader.batches.push_back(b.clone());
        }
        for serial in lost {
            self.finish(serial, disposition::LOST, reason::OVERFLOW);
        }
        console
    }
}
fn row_mut(ledger: &mut VecDeque<Row>, serial: u64) -> Option<&mut Row> {
    let first = ledger.front()?.event.serial;
    ledger.get_mut(usize::try_from(serial.checked_sub(first)?).ok()?)
}
static STATE: spin::Mutex<State> = spin::Mutex::new(State::new());

// Allocate with the open description, then transfer ownership at final close.
// Descriptor teardown can run under PM; it must neither wait on STATE nor
// allocate storage for the deferred work.
struct Retirement {
    reader: Option<u64>,
    device: Option<u32>,
    next: Option<Box<Retirement>>,
}
static RETIRED_INPUT: crate::irq_safe_mutex::IrqSafeMutex<Option<Box<Retirement>>> =
    crate::irq_safe_mutex::IrqSafeMutex::new(None);

fn retire_descriptors() {
    loop {
        let next = {
            let mut pending = RETIRED_INPUT.lock();
            pending.take().map(|mut entry| {
                *pending = entry.next.take();
                entry
            })
        };
        let Some(entry) = next else { break };
        let mut s = STATE.lock();
        if let Some(id) = entry.reader {
            s.readers.remove(&id);
            s.release_grab(id, disposition::LOST, reason::COMPOSITOR_DIED);
        }
        if let Some(id) = entry.device {
            s.removals.push_back(id);
        }
    }
}

fn batch(records: Vec<Record>) -> Batch {
    let mut bytes = Vec::new();
    for r in &records {
        encode(&r.event, &mut bytes);
        bytes.extend_from_slice(&r.payload);
    }
    Batch { records, bytes }
}
fn encode(e: &Event, bytes: &mut Vec<u8>) {
    bytes.extend_from_slice(&e.serial.to_le_bytes());
    bytes.extend_from_slice(&e.time_ns.to_le_bytes());
    bytes.extend_from_slice(&e.device.to_le_bytes());
    bytes.extend_from_slice(&e.kind.to_le_bytes());
    bytes.extend_from_slice(&e.code.to_le_bytes());
    bytes.extend_from_slice(&e.value.to_le_bytes());
    bytes.extend_from_slice(&e.flags.to_le_bytes());
}
fn decode(b: &[u8]) -> Event {
    Event {
        serial: u64::from_le_bytes(b[0..8].try_into().unwrap()),
        time_ns: u64::from_le_bytes(b[8..16].try_into().unwrap()),
        device: u32::from_le_bytes(b[16..20].try_into().unwrap()),
        kind: u16::from_le_bytes(b[20..22].try_into().unwrap()),
        code: u16::from_le_bytes(b[22..24].try_into().unwrap()),
        value: i32::from_le_bytes(b[24..28].try_into().unwrap()),
        flags: u32::from_le_bytes(b[28..32].try_into().unwrap()),
    }
}
fn wake_readers() {
    let queues: Vec<_> = STATE
        .lock()
        .readers
        .values()
        .map(|r| r.wake.clone())
        .collect();
    for q in queues {
        q.wake_up_deferred();
    }
}

/// An open description shared by dup and fork. Descriptor and active-I/O
/// references own its lifetime; final Drop releases reader custody.
pub struct InputFile {
    reader: Option<u64>,
    device: spin::Mutex<Option<u32>>,
    read_gate: spin::Mutex<()>,
    wake: Arc<WaitQueueHead>,
    retirement: Option<Box<Retirement>>,
}
impl core::fmt::Debug for InputFile {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("InputFile")
            .field("reader", &self.reader)
            .finish()
    }
}
impl InputFile {
    pub fn open(injector: bool) -> Arc<dyn IpcFile> {
        retire_descriptors();
        let wake = Arc::new(WaitQueueHead::new());
        let reader = if injector {
            None
        } else {
            let mut s = STATE.lock();
            let id = s.next_reader;
            s.next_reader += 1;
            s.readers.insert(
                id,
                Reader {
                    batches: VecDeque::new(),
                    events: 0,
                    dropped: None,
                    wake: wake.clone(),
                },
            );
            Some(id)
        };
        Arc::new(Self {
            reader,
            device: spin::Mutex::new(None),
            read_gate: spin::Mutex::new(()),
            wake,
            retirement: Some(Box::new(Retirement {
                reader,
                device: None,
                next: None,
            })),
        })
    }
    pub fn ioctl(&self, request: u64, arg: u64) -> Result<u64, u64> {
        retire_descriptors();
        let reader = self.reader.ok_or(ENOTTY as u64)?;
        if request == ioctl::CAPABILITIES {
            let mut query = userptr::copy_from_user::<input_abi::Capabilities>(arg as *const _)?;
            if query.count as usize > MAX_CAPABILITIES {
                return Err(EINVAL as u64);
            }
            // Copy user memory after releasing the stream lock. The snapshot
            // remains valid if the device detaches during the copy.
            let entries = STATE.lock().devices.get(&query.device)
                .ok_or(ENODEV as u64)?.capabilities.clone();
            let capacity = query.count as usize;
            query.count = entries.len() as u32;
            if capacity >= entries.len() {
                for (index, entry) in entries.iter().enumerate() {
                    let address = query.entries.checked_add((index * core::mem::size_of::<Capability>()) as u64)
                        .ok_or(EFAULT as u64)?;
                    userptr::copy_to_user(address as *mut Capability, entry)?;
                }
            }
            userptr::copy_to_user(arg as *mut input_abi::Capabilities, &query)?;
            return Ok(0);
        }
        let caller = crate::window::Caller::current().map_err(|e| e as u64)?;
        let selection = if request == ioctl::GRAB {
            let selection: input_abi::Selection = userptr::copy_from_user(arg as *const _)?;
            if selection.reserved != 0 || selection.count as usize > MAX_CAPABILITIES {
                return Err(EINVAL as u64);
            }
            let mut ids = Vec::new();
            for n in 0..selection.count as u64 {
                let addr = selection.devices.checked_add(n * 4).ok_or(EFAULT as u64)?;
                let id = userptr::copy_from_user(addr as *const u32)?;
                if ids.contains(&id) {
                    return Err(EINVAL as u64);
                }
                ids.push(id);
            }
            Some(ids)
        } else {
            None
        };
        let outcome = if request == ioctl::DISPOSITION {
            Some(userptr::copy_from_user::<Disposition>(arg as *const _)?)
        } else {
            None
        };
        let _read = self.read_gate.lock();
        // Keep claim authority stable until the input transaction commits.
        let reg = crate::window::REGISTRY.lock();
        if !reg.is_compositor(&caller) {
            return Err(EPERM as u64);
        }
        let epoch = reg.claim.as_ref().unwrap().epoch;
        match request {
            ioctl::GRAB => {
                let ids = selection.unwrap();
                let mut s = STATE.lock();
                if s.grab.as_ref().is_some_and(|g| g.reader != reader) {
                    return Err(EBUSY as u64);
                }
                if ids.iter().any(|id| !s.devices.contains_key(id)) {
                    return Err(ENODEV as u64);
                }
                let pending: Vec<_> = s
                    .readers
                    .get(&reader)
                    .unwrap()
                    .batches
                    .iter()
                    .flat_map(|b| b.records.iter().map(|r| r.event.serial))
                    .collect();
                for serial in pending {
                    s.finish(serial, disposition::REJECTED, reason::GRAB);
                }
                let r = s.readers.get_mut(&reader).unwrap();
                r.batches.clear();
                r.events = 0;
                r.dropped = None;
                let regrab = s.grab.is_some();
                if regrab {
                    s.release_grab(reader, disposition::REJECTED, reason::GRAB);
                }
                for (id, d) in &mut s.devices {
                    if ids.is_empty() || ids.contains(id) {
                        d.held.clear();
                        d.modifiers = 0;
                        d.reserved = 0;
                    }
                }
                s.grab = Some(Grab {
                    reader,
                    devices: ids,
                    holder: caller.tgid,
                    epoch,
                });
                // Publish the service wake after releasing STATE; the wait
                // predicate observes the wake generation without taking STATE.
                drop(s);
                drop(reg);
                if regrab {
                    wake_service();
                }
                Ok(0)
            }
            ioctl::UNGRAB => {
                let mut s = STATE.lock();
                if s.grab.as_ref().is_none_or(|g| g.reader != reader) {
                    return Err(EPERM as u64);
                }
                s.release_grab(reader, disposition::REJECTED, reason::GRAB);
                drop(s);
                drop(reg);
                wake_readers();
                wake_service();
                Ok(0)
            }
            ioctl::DISPOSITION => {
                let d = outcome.unwrap();
                if d.reserved != 0
                    || d.serial == 0
                    || !(1..=5).contains(&d.disposition)
                    || d.reason > reason::COMPOSITOR_DIED
                    || ((d.disposition == disposition::DELIVERED
                        || d.disposition == disposition::CLIENT_REJECTED)
                        && d.window == 0)
                {
                    return Err(EINVAL as u64);
                }
                if d.window != 0 && !reg.windows.contains_key(&d.window) {
                    return Err(ENOENT as u64);
                }
                let mut s = STATE.lock();
                if !s.grab.as_ref().is_some_and(|g| {
                    g.reader == reader && g.holder == caller.tgid && g.epoch == epoch
                }) {
                    return Err(EPERM as u64);
                }
                let row = row_mut(&mut s.ledger, d.serial).ok_or(ENOENT as u64)?;
                if row.reader != Some(reader) {
                    return Err(EPERM as u64);
                }
                if d.derived != 0 {
                    // A derived outcome cannot cover a missing source outcome.
                    if s.derivations.contains_key(&(d.serial, d.derived)) {
                        return Err(EALREADY as u64);
                    }
                    s.derivations.insert((d.serial, d.derived), d);
                    while s.derivations.len() > input_abi::READER_EVENTS {
                        s.derivations.pop_first();
                    }
                    return Ok(0);
                }
                if row.outcome.is_some() {
                    return Err(EALREADY as u64);
                }
                row.outcome = Some(d);
                let event = row.event;
                if event.kind == kind::KEY && d.disposition == disposition::DELIVERED {
                    if event.value == 1 {
                        // Recipient state outlives the physical edge and device.
                        s.recipients.insert(
                            (event.device, event.serial),
                            Held {
                                event,
                                window: d.window,
                            },
                        );
                    } else if event.value == 0 {
                        s.recipients.retain(|(device, serial), held| {
                            !(*device == event.device
                                && *serial < event.serial
                                && held.event.code == event.code
                                && held.window == d.window)
                        });
                    }
                }
                // Publish the service wake after releasing STATE; the wait
                // predicate observes the wake generation without taking STATE.
                drop(s);
                drop(reg);
                wake_service();
                Ok(0)
            }
            _ => Err(ENOTTY as u64),
        }
    }
    fn create(&self, bytes: &[u8]) -> Result<u32, u64> {
        if bytes.len() < 16 || bytes[..8] != input_abi::CREATE_MAGIC {
            return Err(EINVAL as u64);
        }
        let flags = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        let n = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        if flags & !input_abi::TIME_INJECTOR != 0
            || n > MAX_CAPABILITIES
            || bytes.len() != 16 + n * 12
        {
            return Err(EINVAL as u64);
        }
        let mut capabilities: Vec<Capability> = Vec::new();
        for b in bytes[16..].chunks_exact(12) {
            let c = Capability {
                kind: u16::from_le_bytes(b[..2].try_into().unwrap()),
                code: u16::from_le_bytes(b[2..4].try_into().unwrap()),
                minimum: i32::from_le_bytes(b[4..8].try_into().unwrap()),
                maximum: i32::from_le_bytes(b[8..12].try_into().unwrap()),
            };
            let valid = match c.kind {
                kind::KEY => {
                    (1..=255).contains(&c.code)
                        || (key::BUTTON_LEFT..=key::BUTTON_MIDDLE).contains(&c.code)
                }
                kind::ABS => c.code <= axis::Y && c.minimum < c.maximum,
                kind::REL => [axis::X, axis::Y, axis::HWHEEL, axis::WHEEL].contains(&c.code),
                kind::TEXT => {
                    c.code == 0 && c.minimum >= 1 && c.maximum <= input_abi::TEXT_LIMIT as i32
                }
                _ => false,
            };
            if !valid
                || c.minimum > c.maximum
                || capabilities
                    .iter()
                    .any(|v| (v.kind, v.code) == (c.kind, c.code))
            {
                return Err(EINVAL as u64);
            }
            capabilities.push(c);
        }
        let mut s = STATE.lock();
        let id = s.next_device;
        s.next_device = id.checked_add(1).ok_or(ENOSPC as u64)?;
        s.devices.insert(
            id,
            Device {
                capabilities,
                injector_time: flags != 0,
                last_time: 0,
                modifiers: 0,
                reserved: 0,
                held: BTreeMap::new(),
            },
        );
        Ok(id)
    }
    fn inject(&self, id: u32, bytes: &[u8]) -> Result<(), u64> {
        let mut s = STATE.lock();
        let device = s.devices.get(&id).ok_or(ENODEV as u64)?;
        let mut at = 0;
        let mut records = Vec::new();
        let mut stamp = device.last_time;
        let now = crate::window::events::now_ns();
        while at < bytes.len() {
            if bytes.len() - at < 32 || records.len() == MAX_BATCH {
                return Err(EINVAL as u64);
            }
            let mut e = decode(&bytes[at..at + 32]);
            at += 32;
            if e.serial != 0
                || e.device != 0
                || e.flags != 0
                || (device.injector_time && e.time_ns < stamp)
            {
                return Err(EINVAL as u64);
            }
            if device.injector_time {
                stamp = e.time_ns;
            } else {
                e.time_ns = now;
            }
            let length = match (e.kind, e.code) {
                (kind::SYN, syn::REPORT) if e.value == 0 => 0,
                (kind::SYN, syn::SCENE) if e.value == 32 && records.is_empty() => 32,
                (kind::SYN, _) => return Err(EINVAL as u64),
                _ => {
                    let c = device
                        .capabilities
                        .iter()
                        .find(|c| c.kind == e.kind && c.code == e.code)
                        .ok_or(EINVAL as u64)?;
                    if e.kind == kind::KEY {
                        if !(0..=2).contains(&e.value) {
                            return Err(EINVAL as u64);
                        }
                    } else if e.value < c.minimum || e.value > c.maximum {
                        return Err(EINVAL as u64);
                    }
                    if e.kind == kind::TEXT {
                        if !(1..=256).contains(&e.value) {
                            return Err(EINVAL as u64);
                        }
                        e.value as usize
                    } else {
                        0
                    }
                }
            };
            if bytes.len() - at < length {
                return Err(EINVAL as u64);
            }
            let payload = bytes[at..at + length].to_vec();
            at += length;
            if e.kind == kind::TEXT && core::str::from_utf8(&payload).is_err() {
                return Err(EINVAL as u64);
            }
            if e.kind == kind::SYN
                && e.code == syn::SCENE
                && u64::from_le_bytes(payload[24..32].try_into().unwrap()) == 0
            {
                return Err(EINVAL as u64);
            }
            let ended = e.kind == kind::SYN && e.code == syn::REPORT;
            if ended && at != bytes.len() {
                return Err(EINVAL as u64);
            }
            e.flags = input_abi::SYNTHETIC;
            records.push(Record { event: e, payload });
        }
        if records.is_empty()
            || !records
                .last()
                .is_some_and(|r| r.event.kind == kind::SYN && r.event.code == syn::REPORT)
        {
            return Err(EINVAL as u64);
        }
        s.reserve_ledger(records.len())?;
        s.devices.get_mut(&id).unwrap().last_time = stamp;
        let console = s.publish(id, records);
        drop(s);
        wake_readers();
        for e in console {
            crate::console::keys::key(
                usize::MAX,
                e.code,
                e.value,
                e.modifiers,
                e.reserved,
                crate::tty::InputSource::Other,
            );
        }
        Ok(())
    }
}
impl IpcFile for InputFile {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
    fn fill_stat(&self, st: &mut crate::syscall::fs::Stat) {
        let device = if self.reader.is_some() {
            crate::fs::devfs::DeviceType::InputEvents
        } else {
            crate::fs::devfs::DeviceType::Uinput
        };
        st.st_dev = 0xffff0002;
        st.st_ino = device.inode();
        st.st_rdev =
            crate::fs::devfs::lookup_by_inode(device.inode()).map_or(0, |node| node.rdev());
        st.st_mode = crate::syscall::fs::S_IFCHR | 0o600;
        st.st_nlink = 1;
        st.st_blksize = 32;
    }
    fn poll_events(&self) -> i16 {
        match self.reader {
            Some(id) => {
                if STATE
                    .lock()
                    .readers
                    .get(&id)
                    .is_some_and(|r| !r.batches.is_empty())
                {
                    1
                } else {
                    0
                }
            }
            None => 4 | if self.device.lock().is_some() { 1 } else { 0 },
        }
    }
    fn poll_queue(&self) -> Option<Arc<WaitQueueHead>> {
        Some(self.wake.clone())
    }
    fn read(&self, buf: u64, count: usize) -> Result<u64, u64> {
        self.read_with_status(buf, count, false)
    }
    fn read_with_status(&self, buf: u64, count: usize, nonblocking: bool) -> Result<u64, u64> {
        let Some(id) = self.reader else {
            if count < 4 {
                return Err(EINVAL as u64);
            }
            let device = (*self.device.lock()).ok_or(EAGAIN as u64)?;
            userptr::copy_to_user(buf as *mut u32, &device)?;
            return Ok(4);
        };
        if count < 32 {
            return Err(EINVAL as u64);
        }
        loop {
            let gate = self.read_gate.lock();
            let taken = {
                let mut s = STATE.lock();
                let r = s.readers.get_mut(&id).ok_or(EBADF as u64)?;
                if let Some(b) = r.batches.front() {
                    let prefix = usize::from(r.dropped.is_some()) * 32;
                    if count < prefix + b.bytes.len() {
                        return Err(EINVAL as u64);
                    }
                    let mut bytes = Vec::new();
                    let dropped = r.dropped.take();
                    if let Some(e) = dropped {
                        encode(&e, &mut bytes);
                    }
                    let mut batches = Vec::new();
                    while r
                        .batches
                        .front()
                        .is_some_and(|b| bytes.len() + b.bytes.len() <= count)
                    {
                        let b = r.batches.pop_front().unwrap();
                        bytes.extend_from_slice(&b.bytes);
                        batches.push(b);
                    }
                    Some((bytes, batches, dropped))
                } else {
                    None
                }
            };
            if let Some((bytes, batches, dropped)) = taken {
                let result = userptr::write_user_bytes(buf, bytes.as_ptr(), bytes.len());
                let mut s = STATE.lock();
                let r = s.readers.get_mut(&id).ok_or(EBADF as u64)?;
                if result.is_ok() {
                    r.events -= batches.iter().map(|b| b.records.len()).sum::<usize>();
                    for batch in &batches {
                        for record in &batch.records {
                            crate::window::pipeline::record(2, record.event.device as u64, id, record.event.serial);
                        }
                    }
                } else {
                    for b in batches.into_iter().rev() {
                        r.batches.push_front(b);
                    }
                    if dropped.is_some() {
                        r.dropped = dropped;
                    }
                }
                result?;
                return Ok(bytes.len() as u64);
            }
            drop(gate);
            if nonblocking {
                return Err(EAGAIN as u64);
            }
            let Some(_) = self
                .wake
                .prepare_to_wait(crate::task::thread::ThreadState::BlockedOnIO)
            else {
                return Err(ENOMEM as u64);
            };
            if let Some(error) = crate::syscall::check_signals_for_restartable_wait() {
                self.wake.finish_wait();
                return Err(error as u64);
            }
            if self.poll_events() & 1 == 0 {
                crate::task::waitqueue::schedule_current_wait();
            }
            self.wake.finish_wait();
        }
    }
    fn write(&self, bytes: &[u8], _append: bool) -> Result<u64, u64> {
        if self.reader.is_some() {
            return Err(EBADF as u64);
        }
        let mut slot = self.device.lock();
        match *slot {
            None => *slot = Some(self.create(bytes)?),
            Some(id) => self.inject(id, bytes)?,
        }
        self.wake.wake_up_deferred();
        Ok(bytes.len() as u64)
    }
}
impl Drop for InputFile {
    fn drop(&mut self) {
        if let Some(mut entry) = self.retirement.take() {
            entry.device = *self.device.get_mut();
            let mut pending = RETIRED_INPUT.lock();
            entry.next = pending.take();
            *pending = Some(entry);
        }
        wake_service();
    }
}

/// A snapshot of source outcomes; pending gaps are deliberately absent.
pub fn ledger() -> alloc::string::String {
    use core::fmt::Write;
    let rows: Vec<_> = STATE
        .lock()
        .ledger
        .iter()
        .filter_map(|row| row.outcome.map(|d| (row.event.serial, d)))
        .collect();
    let mut out = alloc::string::String::new();
    for (serial, mut d) in rows {
        if d.disposition == disposition::DELIVERED {
            let reg = crate::window::REGISTRY.lock();
            if let Some(w) = reg.windows.get(&d.window) {
                if w.meta.rejected[..(w.meta.rejected_count as usize).min(8)].contains(&serial) {
                    d.disposition = disposition::CLIENT_REJECTED;
                }
            }
        }
        let name = match d.disposition {
            1 => "wm-handled",
            2 => "delivered",
            3 => "rejected",
            4 => "client-rejected",
            _ => "lost",
        };
        let why = match d.reason {
            0 => "none",
            1 => "no-focus",
            2 => "minimized",
            3 => "not-selected",
            4 => "stale-scene",
            5 => "target-gone",
            6 => "grab",
            7 => "stale-layout",
            8 => "gesture-cancelled",
            9 => "overflow",
            10 => "ring-full",
            _ => "compositor-died",
        };
        let _ = writeln!(out, "{serial} {name} {} {why}", d.window);
    }
    out
}

static RELEASE_WORK: WaitQueueHead = WaitQueueHead::new();
static SERVICE_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Start before userspace memory accounting begins. The service is idle until
/// a descriptor's final close publishes releases, including closes under PM.
pub fn init() -> Result<(), &'static str> {
    STATE
        .lock()
        .ledger
        .try_reserve_exact(LEDGER_EVENTS)
        .map_err(|_| "Failed to reserve input ledger")?;
    crate::task::kthread::kthread_run(release_service, "input-releases")
        .map(|_| ())
        .map_err(|_| "Failed to start input release service")
}
fn release_service() {
    use crate::task::{thread::ThreadState, waitqueue::PrepareOutcome};
    loop {
        crate::per_cpu::preempt_disable();
        let generation = SERVICE_GENERATION.load(Ordering::Acquire);
        retire_descriptors();
        while super::hardware::drain_one() {}
        while super::host::drain_one() {}
        loop {
            let console = {
                let mut s = STATE.lock();
                if !s.removal_ready() {
                    break;
                }
                let id = *s.removals.front().unwrap();
                let count = s.removal_count(id);
                if s.reserve_ledger(count).is_err() {
                    break;
                }
                s.removals.pop_front();
                s.remove_device(id)
            };
            wake_readers();
            for e in console {
                crate::console::keys::key(
                    usize::MAX,
                    e.code,
                    e.value,
                    e.modifiers,
                    e.reserved,
                    crate::tty::InputSource::Other,
                );
            }
        }
        loop {
            let next = STATE.lock().releases.pop_front();
            let Some(release) = next else {
                break;
            };
            let (serial, index) = {
                let mut s = STATE.lock();
                let index = (1..=2 * input_abi::READER_EVENTS as u32 + 1)
                    .find(|index| !s.derivations.contains_key(&(release.source.serial, *index)))
                    .expect("bounded derived tables leave an unused index");
                let serial = s.next_derived;
                s.next_derived += 1;
                (serial, index)
            };
            let wake = {
                let mut reg = crate::window::REGISTRY.lock();
                reg.windows.get_mut(&release.window).map(|w| {
                    let mut event = crate::window::events::kernel_event(
                        if release.source.code >= key::BUTTON_LEFT {
                            breengel_abi::event_kind::POINTER_BUTTON
                        } else {
                            breengel_abi::event_kind::KEY_RELEASE
                        },
                    );
                    event.serial = serial;
                    if release.source.code >= key::BUTTON_LEFT {
                        event.button = release.source.code as u32;
                    } else {
                        event.keycode = release.source.code as u32;
                    }
                    let queued = w.ring.push(event);
                    (w.waits.events.clone(), queued)
                })
            };
            let delivered = wake.as_ref().is_some_and(|(_, queued)| *queued);
            {
                let mut s = STATE.lock();
                s.derivations.insert(
                    (release.source.serial, index),
                    Disposition {
                        serial: release.source.serial,
                        window: release.window,
                        disposition: if delivered {
                            disposition::DELIVERED
                        } else {
                            disposition::LOST
                        },
                        reason: if delivered {
                            reason::NONE
                        } else if wake.is_some() {
                            reason::RING_FULL
                        } else {
                            reason::TARGET_GONE
                        },
                        derived: index,
                        reserved: 0,
                    },
                );
                while s.derivations.len() > input_abi::READER_EVENTS {
                    s.derivations.pop_first();
                }
            }
            if let Some((queue, _)) = wake {
                queue.wake_up();
            }
        }
        // The serial IRQ only increments a generation. A bounded timed wait
        // services it even if its arrival races wait publication; no IRQ path
        // needs a waitqueue or scheduler lock. Ordinary boots remain event-driven.
        let host_deadline = super::host::armed().then(|| crate::window::events::now_ns().saturating_add(1_000_000));
        let outcome = RELEASE_WORK.prepare_to_wait_checked(ThreadState::BlockedOnIO, host_deadline, || {
            // IRQ wakes may interrupt a task holding STATE. Keep this predicate
            // lock-free; the generation detects work published since draining.
            SERVICE_GENERATION.load(Ordering::Acquire) == generation
        });
        match outcome {
            PrepareOutcome::Queued => {
                crate::task::waitqueue::schedule_current_wait();
                RELEASE_WORK.finish_wait();
            }
            PrepareOutcome::Mismatch => {}
            PrepareOutcome::OutOfMemory => crate::task::scheduler::yield_current(),
            PrepareOutcome::PublishFailed => {
                panic!("Input release service could not publish its wait")
            }
        }
        crate::per_cpu::preempt_enable();
    }
}

/// The serial receive interrupt must not enter a waitqueue or scheduler lock.
pub(super) fn notify_host() {
    SERVICE_GENERATION.fetch_add(1, Ordering::Release);
}

pub fn wake_service() {
    SERVICE_GENERATION.fetch_add(1, Ordering::Release);
    RELEASE_WORK.wake_up_deferred();
}

/// IRQ-safe driver publication; the service drains the fixed-storage handoff.
pub use super::hardware::report as publish_hardware;

/// The shared registration interface. USB HID and ARM virtio-input supply the
/// same driver-neutral description from their task-context initialization paths.
pub fn register_hardware(
    description: super::hardware::Description,
) -> Result<super::hardware::Device, &'static str> {
    crate::arch_without_interrupts(|| {
        let mut s = STATE.lock();
        let id = s.next_hardware;
        if id as usize >= super::hardware::DEVICE_LIMIT {
            return Err("hardware input identities exhausted");
        }
        s.next_hardware += 1;
        s.devices.insert(
            id,
            Device {
                capabilities: description.capabilities,
                injector_time: false,
                last_time: 0,
                modifiers: 0,
                reserved: 0,
                held: BTreeMap::new(),
            },
        );
        Ok(super::hardware::Device {
            id,
            keyboard: description.keyboard,
        })
    })
}

pub(super) fn drain_hardware(
    device: super::hardware::Device,
    events: &[super::hardware::Event],
    time_ns: u64,
    dropped: bool,
) -> bool {
    let mut s = STATE.lock();
    if s.reserve_ledger(events.len() + usize::from(dropped))
        .is_err()
    {
        return false;
    }
    let prefix = dropped.then_some(super::hardware::Event {
        kind: kind::SYN,
        code: syn::DROPPED,
        value: 0,
    });
    if let Some(keyboard) = device.keyboard {
        for e in prefix.iter().chain(events) {
            if e.kind == kind::SYN && e.code == syn::DROPPED {
                super::reset_keyboard(keyboard);
            }
            if e.kind == kind::KEY {
                super::observe_key(keyboard, e.code, e.value);
            }
        }
    }
    let records = prefix
        .iter()
        .chain(events)
        .map(|e| Record {
            event: Event {
                kind: e.kind,
                code: e.code,
                value: e.value,
                time_ns,
                ..Event::default()
            },
            payload: Vec::new(),
        })
        .collect();
    let console = s.publish(device.id, records);
    drop(s);
    wake_readers();
    deliver_hardware_console(device, console);
    true
}
fn deliver_hardware_console(device: super::hardware::Device, console: Vec<ConsoleKey>) {
    let Some(keyboard) = device.keyboard else {
        return;
    };
    for e in console {
        let sent = crate::console::keys::key(
            keyboard.index(),
            e.code,
            e.value,
            e.modifiers,
            e.reserved,
            keyboard.source(),
        );
        if matches!(keyboard, super::Device::UsbBootKeyboard(_)) {
            crate::drivers::hid_boot::console_delivery(sent);
        }
    }
}
pub(super) fn remove_hardware(device: super::hardware::Device) -> bool {
    let mut s = STATE.lock();
    let count = s.removal_count(device.id);
    if s.reserve_ledger(count).is_err() {
        return false;
    }
    let console = s.remove_device(device.id);
    drop(s);
    if let Some(keyboard) = device.keyboard {
        super::reset_keyboard(keyboard);
    }
    wake_readers();
    deliver_hardware_console(device, console);
    true
}
