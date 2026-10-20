//! metal-boot-manager: the Breenix boot manager on the R240's EFI system partition (docs/metal/README.md, "Boot
//! manager and persistent install").
//!
//! It starts first in the firmware's boot order, asks the lab (the boot host) whether a test boot is pending, and
//! either starts the network test path through the firmware's HTTP boot option or boots a kernel slot of the
//! persistent install. Its menu is drawn on the screen and written to COM1: Breenix (the current slot), Breenix
//! (previous kernel), Test loop (network) and Firmware setup, with arrow keys, Enter and a countdown to the default.
//! Started over the network instead (probe mode), it draws the menu, asks the lab, reports what the firmware offers
//! and halts without writing anything.
//!
//! Clean room: written only from the UEFI 2.10 specification. Progress lines start `breenix-manager:`.
#![no_std]
#![no_main]

mod clock;
mod lab;
mod options;
mod screen;
mod slots;

use core::fmt::Write;
use core::ptr::null_mut;

use metal_efi::com1::{self, Vendor};
use metal_efi::files::{self, Root};
use metal_efi::say;
use metal_efi::uefi::*;

use clock::Clock;
use lab::{Answer, Entry, SlotAction};
use screen::{Line, Screen};
use slots::{Config, Note, TestPath};

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    say!("panic: {}", info.message());
    com1::halt()
}

#[no_mangle]
extern "efiapi" fn efi_main(image: Handle, system: *const SystemTable) -> Status {
    com1::set_prefix("breenix-manager: ");
    // SAFETY: the firmware passes this image's handle and its system table, valid while boot services run.
    unsafe { run(image, system) }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Item {
    Current,
    Previous,
    TestLoop,
    FirmwareSetup,
}

const ITEMS: [Item; 4] = [Item::Current, Item::Previous, Item::TestLoop, Item::FirmwareSetup];
/// Rows of the screen: the title, the entries, the lab's status, a second status line and the keys.
const TITLE_ROW: usize = 0;
const FIRST_ITEM_ROW: usize = 2;
const STATUS_ROW: usize = 8;
const DETAIL_ROW: usize = 9;
const COUNTDOWN_ROW: usize = 11;
const KEYS_ROW: usize = 13;
const TEST_COUNTDOWN: u32 = 3;

struct Manager<'a> {
    boot: &'a BootServices,
    runtime: &'a RuntimeServices,
    system: *const SystemTable,
    image: Handle,
    screen: Option<Screen<'a>>,
    esp: Option<Root>,
    current: Note,
    previous: Note,
    selected: usize,
    status: Line,
    detail: Line,
    countdown: Line,
    /// The HTTP boot port's device path (a site value, kept off COM1 and the screen), the HTTP boot option, and how the test loop
    /// is started.
    port: [u8; slots::PATH_MAX],
    port_len: usize,
    http_option: Option<u16>,
    test_path: TestPath,
    setup_offered: bool,
}

fn line(arguments: core::fmt::Arguments) -> Line {
    let mut out = Line::new();
    let _ = out.write_fmt(arguments);
    out
}

impl<'a> Manager<'a> {
    fn available(&self, item: Item) -> bool {
        match item {
            Item::Current => self.current.present,
            Item::Previous => self.previous.present,
            Item::TestLoop => self.port_len > 0 && (self.test_path == TestPath::LoadFile || self.http_option.is_some()),
            Item::FirmwareSetup => true,
        }
    }

    fn item_text(&self, item: Item) -> Line {
        match item {
            Item::Current if self.current.present => line(format_args!("Breenix ({})", self.current.short())),
            Item::Current => line(format_args!("Breenix (no kernel installed)")),
            Item::Previous if self.previous.present => {
                line(format_args!("Breenix (previous kernel, {})", self.previous.short()))
            }
            Item::Previous => line(format_args!("Breenix (previous kernel: none kept)")),
            Item::TestLoop => line(format_args!("Test loop (network)")),
            Item::FirmwareSetup if self.setup_offered => line(format_args!("Firmware setup")),
            Item::FirmwareSetup => line(format_args!("Firmware setup (restarts; press F2 at the firmware logo)")),
        }
    }

    /// Lays the rows out, draws them, and writes them to COM1.
    fn show(&mut self, read_back: bool) {
        let mut rows = [screen::Row { text: [b' '; screen::COLS], len: 0, inverted: false }; screen::ROWS];
        rows[TITLE_ROW] = line(format_args!("Breenix boot manager")).row;
        for (index, &item) in ITEMS.iter().enumerate() {
            let text = self.item_text(item);
            let marker = if index == self.selected { "> " } else { "  " };
            let mut row = line(format_args!("{}{}", marker, text.row.as_str())).row;
            row.inverted = index == self.selected;
            if !self.available(item) && index != self.selected {
                row = line(format_args!("  {} (unavailable)", text.row.as_str())).row;
            }
            rows[FIRST_ITEM_ROW + index] = row;
        }
        rows[STATUS_ROW] = self.status.row;
        rows[DETAIL_ROW] = self.detail.row;
        rows[COUNTDOWN_ROW] = self.countdown.row;
        rows[KEYS_ROW] = line(format_args!("Up and Down choose, Enter starts.")).row;
        for row in rows.iter().filter(|row| row.len > 0) {
            say!("menu: {}", row.as_str().trim_end());
        }
        if let Some(screen) = self.screen.as_mut() {
            screen.rows = rows;
            screen.draw();
            if read_back {
                screen.report();
            }
        }
    }

    fn set_status(&mut self, status: Line, detail: Line) {
        self.status = status;
        self.detail = detail;
    }

    /// Waits up to `seconds` for a key, counting down on the screen with `what` and what a key does; an empty result
    /// means the countdown ran out. With `seconds` 0 it waits for one key without a countdown, and halts when the
    /// firmware has no console input.
    unsafe fn wait(&mut self, seconds: u32, what: &str, key: &str) -> Option<InputKey> {
        let con_in = (*self.system).con_in;
        let mut timer: Event = null_mut();
        if (self.boot.create_event)(EVT_TIMER, 0, None, null_mut(), &mut timer) != SUCCESS {
            timer = null_mut();
        } else {
            (self.boot.set_timer)(timer, TIMER_PERIODIC, 10_000_000);
        }
        let mut left = seconds;
        let result = loop {
            if seconds > 0 {
                self.countdown = line(format_args!("{} in {} s; {}", what, left, key));
                self.show(false);
                if left == 0 {
                    break None;
                }
            }
            let events = [if con_in.is_null() { null_mut() } else { (*con_in).wait_for_key }, timer];
            let (first, count) = match (events[0].is_null(), events[1].is_null()) {
                (false, false) => (0, 2),
                (false, true) => (0, 1),
                (true, false) if seconds > 0 => (1, 1),
                _ => {
                    say!("the firmware has no console input to choose an entry with; halting");
                    com1::halt()
                }
            };
            let mut index = 0usize;
            if (self.boot.wait_for_event)(count, events[first..].as_ptr(), &mut index) != SUCCESS {
                break None;
            }
            if first + index == 1 {
                if seconds > 0 {
                    left -= 1;
                }
                continue;
            }
            let mut key = InputKey::default();
            if ((*con_in).read_key_stroke)(con_in, &mut key) == SUCCESS {
                break Some(key);
            }
        };
        if !timer.is_null() {
            (self.boot.close_event)(timer);
        }
        self.countdown = Line::new();
        result
    }

    fn select(&mut self, item: Item) {
        self.selected = ITEMS.iter().position(|&each| each == item).unwrap_or(0);
    }

    /// Starts the selected item; returns only when it could not be started, or returned.
    unsafe fn start(&mut self) -> Status {
        let item = ITEMS[self.selected];
        if !self.available(item) {
            return NOT_FOUND;
        }
        // A boot manager arms the five-minute watchdog before it starts a boot option (section 3.1.2).
        (self.boot.set_watchdog_timer)(300, 0, 0, core::ptr::null());
        let status = match item {
            Item::Current | Item::Previous => {
                let entry = if item == Item::Current { Entry::Current } else { Entry::Previous };
                let note = if entry == Entry::Current { self.current } else { self.previous };
                // The screen as it is at the start, read back once more, so a console capture that began late (an
                // operator's power-on, adopted by the lab) still holds it.
                self.show(true);
                say!("booting {} {}", if entry == Entry::Current { "current" } else { "previous" }, note.full());
                match self.esp.as_ref() {
                    Some(root) => slots::boot(self.boot, self.image, root, entry),
                    None => NOT_FOUND,
                }
            }
            Item::TestLoop => {
                say!("starting the test loop");
                let port = self.port;
                lab::start_test_loop(self.boot, self.runtime, self.image, &port[..self.port_len], self.http_option, self.test_path)
            }
            Item::FirmwareSetup => options::firmware_setup(self.boot, self.runtime),
        };
        (self.boot.set_watchdog_timer)(0, 0, 0, core::ptr::null());
        status
    }

    /// The menu after the lab's answer: keys move the selection, Enter starts it, and a failed start comes back here
    /// with its reason and no countdown.
    unsafe fn menu(&mut self, mut countdown: u32) -> ! {
        loop {
            let what = match ITEMS[self.selected] {
                Item::Current => "Starting Breenix",
                Item::Previous => "Starting the previous kernel",
                Item::TestLoop => "Starting the test loop",
                Item::FirmwareSetup => "Entering firmware setup",
            };
            let outcome = self.wait(countdown, what, "any key stops the countdown.");
            countdown = 0;
            match outcome {
                None => {}
                Some(key) if key.scan_code == SCAN_UP => {
                    self.selected = (self.selected + ITEMS.len() - 1) % ITEMS.len();
                    self.show(false);
                    continue;
                }
                Some(key) if key.scan_code == SCAN_DOWN => {
                    self.selected = (self.selected + 1) % ITEMS.len();
                    self.show(false);
                    continue;
                }
                Some(key) if key.unicode_char == 0x0d => {}
                Some(_) => {
                    self.show(false);
                    continue;
                }
            }
            let status = self.start();
            let failed = self.item_text(ITEMS[self.selected]);
            self.set_status(
                line(format_args!("{} did not start ({:#x}).", failed.row.as_str(), status)),
                line(format_args!("Choose an entry.")),
            );
            say!("{} did not start ({:#x})", failed.row.as_str(), status);
            self.show(true);
        }
    }
}

unsafe fn run(image: Handle, system: *const SystemTable) -> Status {
    let boot = &*(*system).boot_services;
    let runtime = &*(*system).runtime_services;
    // The menu waits for the operator as long as it takes; the firmware's watchdog is armed again before a start.
    (boot.set_watchdog_timer)(0, 0, 0, core::ptr::null());
    say!("started: firmware {} revision {:#x}", Vendor((*system).firmware_vendor), (*system).firmware_revision);
    let clock = Clock::new(boot);
    let con_out = (*system).con_out;
    if !con_out.is_null() {
        ((*con_out).enable_cursor)(con_out, 0);
    }
    let esp = files::own_root(boot, image).ok();
    if let Some(root) = esp.as_ref() {
        if let Err(status) = slots::recover(root) {
            say!("slot recovery failed ({:#x}); returning to firmware", status);
            (boot.set_watchdog_timer)(300, 0, 0, core::ptr::null());
            return status;
        }
    }
    let mut config_text = [0u8; 1024];
    let config: Option<Config> = esp
        .as_ref()
        .and_then(|root| root.read("\\EFI\\BREENIX\\MANAGER.CFG", &mut config_text).ok())
        .and_then(|len| slots::parse_config(&config_text[..len]));
    let mut network_url = [0u8; slots::URL_MAX];
    let network_probe = probe_query_url(boot, image, &mut network_url).is_some();
    if config.is_none() && !network_probe {
        say!("installed configuration unavailable; trying existing slots before returning to firmware");
        (boot.set_watchdog_timer)(300, 0, 0, core::ptr::null());
        if let Some(root) = esp.as_ref() {
            for entry in [Entry::Current, Entry::Previous] {
                let note = slots::note(root, slots::slot_name(entry));
                if note.present {
                    say!("booting {} {}", if entry == Entry::Current { "current" } else { "previous" }, note.full());
                    slots::boot(boot, image, root, entry);
                }
            }
        }
        return LOAD_ERROR;
    }
    let probe = config.is_none() && network_probe;
    let mut manager = Manager {
        boot,
        runtime,
        system,
        image,
        screen: Screen::open(boot, system),
        current: esp.as_ref().map_or(Note::ABSENT, |root| slots::note(root, "CURRENT")),
        previous: esp.as_ref().map_or(Note::ABSENT, |root| slots::note(root, "PREVIOUS")),
        esp,
        selected: 0,
        status: line(format_args!("Asking the lab whether a test boot is pending.")),
        detail: Line::new(),
        countdown: Line::new(),
        port: [0; slots::PATH_MAX],
        port_len: 0,
        http_option: None,
        test_path: TestPath::BootNext,
        setup_offered: options::setup_offered(runtime),
    };
    if let Some(config) = config.as_ref() {
        manager.port[..config.port_len].copy_from_slice(config.port());
        manager.port_len = config.port_len;
        manager.http_option = config.http_option;
        manager.test_path = config.test_path;
        say!("slots: current {}, previous {}", manager.current.full(), manager.previous.full());
    } else {
        say!("probe mode: started without its configuration on a disk; nothing will be written or started");
        // Over the network, this image's own path is the HTTP boot port's, and the option running is the HTTP one.
        manager.port_len = lab::own_path(boot, image, &mut manager.port).unwrap_or(0);
        manager.http_option = options::get_u16(runtime, "BootCurrent");
        manager.test_path = TestPath::LoadFile;
    }
    let mut name = [0u8; 8];
    match manager.http_option {
        Some(number) => say!("the HTTP boot option is {}", options::option_name(number, &mut name)),
        None => say!("no HTTP boot option is known"),
    }
    let mut kinds = [0u8; 96];
    let kinds_len = options::kinds(&manager.port[..manager.port_len], &mut kinds);
    say!("the HTTP boot port's path: {}", core::str::from_utf8(&kinds[..kinds_len]).unwrap_or("?"));
    if let Some(screen) = manager.screen.as_mut() {
        screen.clear();
    } else {
        say!("no graphics output; the menu is on COM1 only");
    }
    manager.show(true);

    let mut probe_url = [0u8; slots::URL_MAX];
    let url: &[u8] = match &config {
        Some(config) => config.url(),
        None => match probe_query_url(boot, image, &mut probe_url) {
            Some(len) => &probe_url[..len],
            None => &[],
        },
    };
    let port_bytes = manager.port;
    let port = &port_bytes[..manager.port_len];
    let answer = if port.is_empty() {
        Answer::None("no network port is known to ask through")
    } else if url.is_empty() {
        Answer::None("no query URL is configured")
    } else {
        lab::ask(boot, image, port, url, &clock, probe)
    };
    let countdown = config.as_ref().map_or(5, |config| config.countdown);
    match answer {
        Answer::Test(id) => {
            say!("lab answered: test {}", id.as_str());
            manager.select(Item::TestLoop);
            manager.set_status(
                line(format_args!("Test boot requested by the lab (boot {}).", id.as_str())),
                Line::new(),
            );
        }
        Answer::Persistent(id, entry, action) => {
            say!(
                "lab answered: persistent {} {}{}",
                id.as_str(),
                if entry == Entry::Current { "current" } else { "previous" },
                match action {
                    SlotAction::None => "",
                    SlotAction::Commit => " commit",
                    SlotAction::Rollback => " rollback",
                }
            );
            manager.select(if entry == Entry::Current { Item::Current } else { Item::Previous });
            manager.set_status(
                line(format_args!("No test boot is pending; session {} holds the machine.", id.as_str())),
                Line::new(),
            );
        }
        Answer::Busy(id) => {
            say!("lab answered: busy {}", id.as_str());
            manager.set_status(
                line(format_args!("The lab holds this machine for boot {}, not yet started;", id.as_str())),
                line(format_args!("the lab will power it off.")),
            );
        }
        Answer::None(why) => {
            say!("lab answered: nothing ({})", why);
            manager.set_status(line(format_args!("The lab did not answer ({}).", why)), Line::new());
        }
    }
    manager.show(true);

    if probe {
        options::report(runtime);
        match options::get_u16(runtime, "BootCurrent") {
            Some(number) => say!("BootCurrent is {}", options::option_name(number, &mut name)),
            None => say!("BootCurrent is not set"),
        }
        lab::report_load_file(boot, port);
        options::report_all(runtime);
        say!("firmware setup through OsIndications: {}", if manager.setup_offered { "offered" } else { "not offered" });
        say!("probe complete");
        com1::halt()
    }

    match answer {
        Answer::Test(id) => {
            // A key does not cancel the lab's boot (it only ends the wait), so the countdown does not offer it.
            if manager.wait(TEST_COUNTDOWN, "Starting the test loop", "a key does not cancel it.").is_some() {
                manager.set_status(
                    line(format_args!("The lab's lease holds this machine for boot {};", id.as_str())),
                    line(format_args!("the test boot goes ahead.")),
                );
                say!("a key was pressed; the lab's lease holds this machine for boot {}, so the test boot goes ahead", id.as_str());
                manager.show(false);
            }
            manager.select(Item::TestLoop);
            let status = manager.start();
            manager.set_status(
                line(format_args!("The test loop did not start ({:#x}).", status)),
                line(format_args!("Choose an entry.")),
            );
            say!("the test loop did not start ({:#x})", status);
            manager.show(true);
            manager.menu(0)
        }
        Answer::Persistent(_, _, action) => {
            let applied = match (action, manager.esp.as_ref()) {
                (SlotAction::None, _) => Ok(()),
                (_, None) => Err(NOT_FOUND),
                (SlotAction::Commit, Some(root)) => slots::commit(root),
                (SlotAction::Rollback, Some(root)) => slots::rollback(root),
            };
            if let Some(root) = manager.esp.as_ref() {
                manager.current = slots::note(root, "CURRENT");
                manager.previous = slots::note(root, "PREVIOUS");
            }
            if let Err(status) = applied {
                say!("the slot action failed ({:#x})", status);
                manager.set_status(line(format_args!("The slot action failed ({:#x}).", status)), Line::new());
                manager.show(true);
                manager.menu(0)
            }
            manager.show(false);
            manager.menu(countdown)
        }
        Answer::Busy(_) => manager.menu(0),
        Answer::None(_) => {
            manager.select(Item::Current);
            manager.show(false);
            manager.menu(countdown)
        }
    }
}

/// In probe mode, the query URL is the boot URL this image was fetched from (the URI node of its device path) with
/// its last path segment replaced by `manager`.
unsafe fn probe_query_url(boot: &BootServices, image: Handle, out: &mut [u8; slots::URL_MAX]) -> Option<usize> {
    let mut loaded = null_mut();
    (boot.handle_protocol)(image, &LOADED_IMAGE, &mut loaded);
    if loaded.is_null() {
        return None;
    }
    let mut path: *mut core::ffi::c_void = null_mut();
    (boot.handle_protocol)((*loaded.cast::<LoadedImage>()).device_handle, &DEVICE_PATH, &mut path);
    if path.is_null() {
        return None;
    }
    let bytes = core::slice::from_raw_parts(path.cast::<u8>(), path_length(path.cast())?);
    let uri = Nodes::new(bytes).find(|(kind, subtype, _)| *kind == node::MESSAGING && *subtype == node::URI)?.2;
    let url = &uri[4..];
    let slash = url.iter().rposition(|&b| b == b'/')?;
    let len = slash + 1 + b"manager".len();
    if !url.starts_with(b"http://") || len >= out.len() {
        return None;
    }
    out[..=slash].copy_from_slice(&url[..=slash]);
    out[slash + 1..len].copy_from_slice(b"manager");
    Some(len)
}
