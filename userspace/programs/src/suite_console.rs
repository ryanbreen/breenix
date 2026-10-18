//! Screen console & virtual terminals: the `console` effort's suite, measuring the contract in
//! `docs/console/contract.md`.
//!
//! One category per measure of an effort milestone (contract section 12): `screen`, `emulation`,
//! `vt-devices`, `vt-switch` and `scrollback`, `chvt` and `getty`, and `keys`.
//!
//! The suite is written before the layers it measures. Each case names the layer it needs and the
//! lane that builds it, and until that layer exists the case fails saying so:
//! `missing <layer> (<lane>): <what the kernel answered>`. It asks the kernel each time, so the
//! reason is measured rather than assumed: the screen layers by opening `/proc/console/check`, the
//! terminal layers by `VT_GETSTATE` on `/dev/console`. When the kernel answers, a case whose check
//! has not been written yet still fails, and says that. No case skips. A lane writes a case's check
//! in its own commit, from the contract, before the implementation it measures.

use libbreenix::error::Error;
use libbreenix::fs;
use libbreenix::io;
use libbreenix::suite::{case, category, fail, suite, CaseResult, Suite};
use libbreenix::syscall::{nr, raw};

static SUITE: Suite = suite("console", "Screen console & virtual terminals", &[
    category("screen", "Console on the screen", &[
        case("tty1-on-screen", "Text written to tty1 is drawn on the screen", screen_tty1_on_screen),
        case("readback-identical", "The displayed pixels read back identical to the console's shadow", screen_readback_identical),
        case("scanout-bound", "The read-back is of the resource bound to the scanout, after a completed flush", screen_scanout_bound),
        case("font-oracle", "Drawn cells match the glyphs the font oracle expects", screen_font_oracle),
        case("size-class", "The font size and grid follow the size rule for the display", screen_size_class),
        case("cursor-drawn", "The cursor band is drawn in the cursor cell and in no other cell", screen_cursor_drawn),
        case("log-on-tty1", "Kernel records at INFO and above appear on tty1 while no login session holds it", screen_log_on_tty1),
        case("log-quiet-in-session", "Only warnings and errors are drawn once a login session holds tty1", screen_log_quiet_in_session),
        case("display-taken-stops-drawing", "The console does not draw while a process owns the display", screen_display_taken_stops_drawing),
        case("display-released-redraws", "Giving the display back redraws the foreground terminal", screen_display_released_redraws),
    ]),
    category("emulation", "Terminal emulation", &[
        case("sgr-colours", "SGR selects the 16 colours and their bright forms", emulation_sgr_colours),
        case("sgr-256-mapped", "256-colour and 24-bit colours map to the nearest of the 16 by the contract's rule", emulation_sgr_256_mapped),
        case("bold-reverse-underline", "Bold draws the bold weight, reverse swaps the pair and underline draws its row", emulation_bold_reverse_underline),
        case("utf8-latin1", "UTF-8 Latin-1 and Latin Extended-A characters draw their glyphs", emulation_utf8_latin1),
        case("utf8-replacement", "Malformed UTF-8 and characters outside the repertoire draw one cell of ?", emulation_utf8_replacement),
        case("box-drawing", "Box-drawing characters draw the generated glyphs at the cell size", emulation_box_drawing),
        case("scroll-region", "DECSTBM limits scrolling to the region", emulation_scroll_region),
        case("insert-delete-line", "IL and DL insert and delete lines within the scroll region", emulation_insert_delete_line),
        case("insert-delete-char", "ICH, DCH and ECH insert, delete and erase characters", emulation_insert_delete_char),
        case("save-restore-cursor", "DECSC and DECRC save and restore the cursor and attributes", emulation_save_restore_cursor),
        case("cursor-hidden", "CSI ?25l hides the cursor and CSI ?25h shows it", emulation_cursor_hidden),
        case("autowrap-off", "With autowrap off, characters past the last column overwrite it", emulation_autowrap_off),
        case("tab-stops", "Tab stops start at each 8th column and HTS and TBC set and clear them", emulation_tab_stops),
        case("alt-screen", "CSI ?1049 switches to a cleared alternate screen and back to the saved one", emulation_alt_screen),
        case("full-reset", "ESC c resets the screen, cursor, attributes, tab stops and modes", emulation_full_reset),
        case("cursor-report", "CSI 6n answers the cursor position on the terminal's input", emulation_cursor_report),
    ]),
    category("vt-devices", "Terminal devices", &[
        case("six-terminals", "tty1 to tty6 exist with their device numbers and open independently", vt_devices_six_terminals),
        case("tty1-is-console", "/dev/console and /dev/tty1 are the same terminal", vt_devices_tty1_is_console),
        case("tty0-is-foreground", "/dev/tty0 reaches the foreground terminal", vt_devices_tty0_is_foreground),
        case("dev-tty-is-ctty", "/dev/tty opens the caller's controlling terminal when it is a VT", vt_devices_dev_tty_is_ctty),
        case("dev-tty-enxio", "/dev/tty fails with ENXIO without a controlling terminal", vt_devices_dev_tty_enxio),
        case("termios-per-vt", "Each terminal keeps its own termios", vt_devices_termios_per_vt),
        case("ctty-per-vt", "Each terminal is the controlling terminal of its own session", vt_devices_ctty_per_vt),
        case("winsize-per-vt", "TIOCGWINSZ reports the size rule's grid on each terminal", vt_devices_winsize_per_vt),
        case("vcs-text", "/dev/vcsN reads the screen text, with ? for characters past Latin-1", vt_devices_vcs_text),
        case("vcsa-attributes", "/dev/vcsaN reads the header and the displayed colour pair of each cell", vt_devices_vcsa_attributes),
        case("background-output-retained", "Output to a terminal in the background is on its screen when it returns", vt_devices_background_output_retained),
        case("background-process-runs", "A process on a terminal in the background keeps running", vt_devices_background_process_runs),
        case("background-read-waits", "A read on a terminal in the background waits and takes keyboard input once it is in the foreground", vt_devices_background_read_waits),
        case("switch-sends-no-signal", "A switch sends no signal and leaves each foreground process group unchanged", vt_devices_switch_sends_no_signal),
    ]),
    category("vt-switch", "Switching & display ownership", &[
        case("activate", "VT_ACTIVATE switches the foreground terminal", vt_switch_activate),
        case("activate-enxio", "VT_ACTIVATE outside 1 to 6 fails with ENXIO", vt_switch_activate_enxio),
        case("waitactive", "VT_WAITACTIVE returns once the terminal is in the foreground", vt_switch_waitactive),
        case("waitactive-after-draw", "VT_WAITACTIVE returns only after the new screen is on the display", vt_switch_waitactive_after_draw),
        case("waitactive-eintr", "A signal interrupts VT_WAITACTIVE with EINTR", vt_switch_waitactive_eintr),
        case("getstate", "VT_GETSTATE reports the foreground terminal and the open ones", vt_switch_getstate),
        case("openqry", "VT_OPENQRY reports the lowest terminal no process has open", vt_switch_openqry),
        case("kdsetmode-graphics", "In KD_GRAPHICS the kernel does not draw the terminal, and KD_TEXT redraws it", vt_switch_kdsetmode_graphics),
        case("kd-owner-exit", "When a KD_GRAPHICS owner exits, its terminal returns to KD_TEXT and is redrawn", vt_switch_kd_owner_exit),
        case("graphics-vt-refuses-switch", "Switching away from a display owner without VT_PROCESS fails with EBUSY", vt_switch_graphics_vt_refuses_switch),
        case("vt-process-release", "A VT_PROCESS owner gets its release signal and the switch waits for VT_RELDISP", vt_switch_vt_process_release),
        case("vt-process-acquire", "A VT_PROCESS owner gets its acquire signal and answers with VT_ACKACQ", vt_switch_vt_process_acquire),
        case("vt-process-refuse", "VT_RELDISP 0 refuses the release and the terminal stays in the foreground", vt_switch_vt_process_refuse),
        case("vt-process-release-timeout", "An owner that does not answer within 3 s keeps the terminal", vt_switch_vt_process_release_timeout),
        case("vt-process-owner-exit", "An owner that exits while a release is pending lets the switch proceed", vt_switch_vt_process_owner_exit),
        case("vt-process-competing", "During a pending release the latest request names the target", vt_switch_vt_process_competing),
        case("vt-process-cancel", "A request for the releasing terminal cancels the switch and signals acquire", vt_switch_vt_process_cancel),
        case("setmode-busy", "VT_SETMODE on a terminal another live process owns fails with EBUSY", vt_switch_setmode_busy),
        case("reldisp-errors", "VT_RELDISP with no answer pending, or from another process, fails", vt_switch_reldisp_errors),
        case("eperm-unprivileged", "A process neither root nor on a terminal gets EPERM for the changing requests", vt_switch_eperm_unprivileged),
        case("switch-readback", "After a switch the displayed pixels read back identical to the new terminal's screen", vt_switch_switch_readback),
        case("switch-log-digest", "A switch logs the terminal number and a digest, not the screen's text", vt_switch_switch_log_digest),
    ]),
    category("scrollback", "Scrollback", &[
        case("history-kept", "Lines scrolled off the screen are kept in the terminal's scrollback", scrollback_history_kept),
        case("history-per-vt", "Each terminal has its own scrollback", scrollback_history_per_vt),
        case("survives-switch", "A terminal's scrollback and view survive switches", scrollback_survives_switch),
        case("view-holds-on-output", "New output does not move a scrolled-back view", scrollback_view_holds_on_output),
        case("key-returns-live", "A key that sends input returns the view to the live screen", scrollback_key_returns_live),
    ]),
    category("chvt", "chvt", &[
        case("chvt-switches", "chvt N switches to terminal N and exits 0", chvt_chvt_switches),
        case("chvt-waits", "chvt returns only once the terminal is in the foreground", chvt_chvt_waits),
        case("chvt-bad-number", "chvt with a number outside 1 to 6 exits 1 with the error", chvt_chvt_bad_number),
    ]),
    category("getty", "A login on each terminal", &[
        case("prompt-each-vt", "Each of tty1 to tty6 shows the login prompt, read back from its own screen", getty_prompt_each_vt),
        case("wrong-password-refused", "A wrong password is refused on a numbered terminal", getty_wrong_password_refused),
        case("login-on-vt", "The per-run account logs in on a numbered terminal", getty_login_on_vt),
        case("logout-reacquires", "After logout the terminal's getty is respawned and prompts again", getty_logout_reacquires),
        case("independent-sessions", "Sessions on two terminals have their own controlling terminals and process groups", getty_independent_sessions),
    ]),
    category("keys", "Switching from the keyboard", &[
        case("alt-fn", "Alt+F1 to F6 switch from a text terminal", keys_alt_fn),
        case("ctrl-alt-fn", "Ctrl+Alt+F1 to F6 switch from a text terminal", keys_ctrl_alt_fn),
        case("ctrl-alt-fn-graphics", "Ctrl+Alt+Fn switches away from a graphics terminal whose owner chose VT_PROCESS", keys_ctrl_alt_fn_graphics),
        case("alt-fn-graphics", "On a graphics terminal Alt+Fn is input to the terminal and does not switch", keys_alt_fn_graphics),
        case("alt-left-right-wrap", "Alt+Left and Alt+Right select the previous and next terminal, wrapping within 1 to 6", keys_alt_left_right_wrap),
        case("alt-f7-ignored", "Alt+F7 to F12 on a text terminal do not switch and send no bytes", keys_alt_f7_ignored),
        case("foreground-owns-input", "Keyboard input reaches only the foreground terminal", keys_foreground_owns_input),
        case("ctrl-c-foreground-only", "Ctrl+C signals only the foreground terminal's foreground process group", keys_ctrl_c_foreground_only),
        case("caps-per-vt", "Caps Lock state is kept per terminal", keys_caps_per_vt),
        case("shift-pageup", "Shift+PageUp and Shift+PageDown move the view by half a screen", keys_shift_pageup),
    ]),
]).defer_display();

/// How the suite asks the kernel whether a layer is there.
#[derive(Clone, Copy)]
enum Probe {
    /// Open `/proc/console/check`, the console core's read-back (contract section 8).
    ConsoleCheck,
    /// `VT_GETSTATE` on `/dev/console` (contract section 4.1).
    VtGetstate,
}

/// A layer a case needs: its name with the lane that builds it, and how to ask for it.
struct Layer {
    name: &'static str,
    probe: Probe,
}

const EMULATION: Layer = Layer { name: "the terminal emulation (V3)", probe: Probe::ConsoleCheck };
const VT: Layer = Layer { name: "the six virtual terminals (V4)", probe: Probe::VtGetstate };
const USER: Layer = Layer { name: "chvt and a getty on each terminal (V4u)", probe: Probe::VtGetstate };
const KEYS: Layer = Layer { name: "the input core and the console keys (V5)", probe: Probe::VtGetstate };

/// `VT_GETSTATE`, from the published `linux/vt.h`: writes a 6-byte `struct vt_stat`.
const VT_GETSTATE: u64 = 0x5603;
/// `O_NOCTTY`, the same on x86-64 and ARM64, so opening the console for the probe does not make it the case's controlling terminal.
const O_NOCTTY: u32 = 0o400;

/// Ask the kernel for a layer: Ok when it answers, else what it answered.
fn probe(probe: Probe) -> Result<(), String> {
    match probe {
        Probe::ConsoleCheck => {
            let fd = fs::open("/proc/console/check", fs::O_RDONLY)
                .map_err(|error| format!("opening /proc/console/check failed with {error}"))?;
            let _ = io::close(fd);
            Ok(())
        }
        Probe::VtGetstate => {
            let fd = fs::open("/dev/console", fs::O_RDONLY | O_NOCTTY)
                .map_err(|error| format!("opening /dev/console failed with {error}"))?;
            let mut state = [0u16; 3];
            // SAFETY: VT_GETSTATE writes one struct vt_stat, three u16s, which `state` holds.
            let ret = unsafe { raw::syscall3(nr::IOCTL, fd.raw(), VT_GETSTATE, state.as_mut_ptr() as u64) };
            let _ = io::close(fd);
            Error::from_syscall(ret as i64)
                .map(|_| ())
                .map_err(|error| format!("VT_GETSTATE on /dev/console failed with {error}"))
        }
    }
}

/// Fail a case whose layer is not there yet, naming the layer and what the kernel answered.
fn missing(layer: &Layer) -> CaseResult {
    match probe(layer.probe) {
        Ok(()) => fail(format!("missing {}: this case's check is not written yet", layer.name)),
        Err(answer) => fail(format!("missing {}: {answer}", layer.name)),
    }
}

// Screen cases use the public root read-back and display/session APIs. No
// test-only record injector: setsid emits INFO and invalid mmap emits WARN.
type Verdict<T = ()> = Result<T, String>;

fn measured(run: impl FnOnce() -> Verdict) -> CaseResult {
    run().map_err(libbreenix::suite::CaseError::Fail)
}

fn require(ok: bool, message: &str) -> Verdict {
    if ok { Ok(()) } else { Err(message.into()) }
}

fn pause() -> Verdict {
    libbreenix::time::sleep_ms(100).map_err(|e| format!("sleep: {e}"))
}

fn screen_check() -> Verdict<String> {
    std::fs::read_to_string("/proc/console/check").map_err(|e| format!("console check: {e}"))
}

fn identical(text: &str) -> Verdict {
    require(text.lines().any(|line| line == "console check: shadow and screen identical"), text)?;
    require(text.lines().any(|line| line.starts_with("console frame: ") && line.ends_with(" complete")), text)
}

fn wait_for_row(marker: &str) -> Verdict<String> {
    for _ in 0..20 {
        let text = screen_check()?;
        if text.lines().any(|line| line.starts_with("console row: ") && line.contains(marker)) {
            identical(&text)?;
            return Ok(text);
        }
        pause()?;
    }
    Err(format!("no displayed row contains {marker}"))
}

fn tty_write(text: &str) -> Verdict {
    let fd = fs::open("/dev/console", fs::O_WRONLY | O_NOCTTY).map_err(|e| format!("console open: {e}"))?;
    let mut bytes = text.as_bytes();
    let result = (|| {
        while !bytes.is_empty() {
            let count = io::write(fd, bytes).map_err(|e| format!("console write: {e}"))?;
            require(count != 0, "console write made no progress")?;
            bytes = &bytes[count..];
        }
        Ok(())
    })();
    let _ = io::close(fd);
    result
}

fn marker(label: &str) -> Verdict<String> {
    let pid = libbreenix::process::getpid().map_err(|e| format!("getpid: {e}"))?;
    Ok(format!("screen-{label}-{}", pid.raw()))
}

fn screen_tty1_on_screen() -> CaseResult {
    measured(|| {
        let marker = marker("tty")?;
        tty_write(&format!("\r\n{marker}\r\n"))?;
        let text = wait_for_row(&marker)?;
        require(text.contains("console foreground: tty1\n"), &text)?;
        let log = std::fs::read_to_string("/proc/kmsg").map_err(|e| format!("root kmsg: {e}"))?;
        require(!log.contains(&marker), "TTY text leaked into the kernel ring")?;
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata("/proc/console/check").map_err(|e| format!("stat check: {e}"))?.permissions().mode();
        require(mode & 0o777 == 0o400, "console/check does not report mode 0400")?;
        let pid = match libbreenix::process::fork().map_err(|e| format!("fork: {e}"))? {
            libbreenix::process::ForkResult::Child => {
                #[cfg(target_arch = "x86_64")]
                const SETUID: u64 = 105;
                #[cfg(target_arch = "aarch64")]
                const SETUID: u64 = 146;
                let dropped = unsafe { raw::syscall1(SETUID, 1000) } as i64 == 0;
                let denied = ["/proc/kmsg", "/proc/console/check"].iter().all(|path| {
                    std::fs::read(path).err().and_then(|e| e.raw_os_error()) == Some(13)
                });
                libbreenix::process::exit(if dropped && denied { 0 } else { 1 });
            }
            libbreenix::process::ForkResult::Parent(pid) => pid,
        };
        let mut status = 0;
        libbreenix::process::waitpid(pid.raw() as i32, &mut status, 0).map_err(|e| format!("waitpid: {e}"))?;
        require(status == 0, "unprivileged process could read a privileged snapshot")
    })
}

fn screen_readback_identical() -> CaseResult {
    measured(|| {
        let marker = marker("pixels")?;
        tty_write(&format!("\r\n{marker} Wm09!?\r\n"))?;
        identical(&wait_for_row(&marker)?)
    })
}

fn screen_scanout_bound() -> CaseResult {
    measured(|| {
        let text = screen_check()?;
        identical(&text)?;
        let source = text.lines().find_map(|line| line.strip_prefix("console scanout: "))
            .ok_or("missing scanout source")?;
        if source.starts_with("scanout memory at ") {
            require(source.contains("display BAR") || source.contains("GOP"), source)
        } else {
            require(source.starts_with("backing after flush ") && source.contains("bound to scanout 0"), source)
        }
    })
}

fn oracle(text: &str) -> Verdict {
    identical(text)?;
    let line = text.lines().find_map(|line| line.strip_prefix("console oracle: cells="))
        .ok_or("missing font oracle")?;
    let (cells, mismatches) = line.split_once(" mismatches=").ok_or("malformed font oracle")?;
    require(cells.parse::<usize>().map_err(|e| e.to_string())? > 0 && mismatches == "0", line)
}

fn screen_font_oracle() -> CaseResult {
    measured(|| {
        let marker = marker("font")?;
        tty_write(&format!("\r\n{marker} ABC xyz 012 []{{}} !?\r\n"))?;
        oracle(&wait_for_row(&marker)?)
    })
}

fn pair(text: &str) -> Verdict<(usize, usize)> {
    let (a, b) = text.split_once('x').ok_or("missing dimension separator")?;
    Ok((a.parse::<usize>().map_err(|e| e.to_string())?, b.parse::<usize>().map_err(|e| e.to_string())?))
}

fn screen_size_class() -> CaseResult {
    measured(|| {
        let text = screen_check()?;
        identical(&text)?;
        let line = text.lines().find_map(|line| line.strip_prefix("console display: ")).ok_or("missing display")?;
        let (width, height) = pair(line.split_whitespace().next().ok_or("missing dimensions")?)?;
        let (_, rest) = line.split_once(", font ").ok_or("missing font")?;
        let (size, rest) = rest.split_once(" (").ok_or("missing cell")?;
        let (cell, grid) = rest.split_once(" pixel cells), grid ").ok_or("missing grid")?;
        let fonts = [(16, 7, 18), (20, 9, 22), (24, 11, 26), (32, 14, 34)];
        let &(wanted, cw, ch) = fonts.iter().rev().find(|&&(_, cw, ch)| width / cw >= 128 && height / ch >= 36)
            .unwrap_or(&fonts[0]);
        require(size.parse::<usize>().map_err(|e| e.to_string())? == wanted, line)?;
        require(pair(cell)? == (cw, ch), line)?;
        require(pair(grid)? == ((width / cw).min(255), (height / ch).min(255)), line)
    })
}

fn screen_cursor_drawn() -> CaseResult {
    measured(|| {
        let marker = marker("cursor")?;
        tty_write(&format!("\r\n{marker}"))?;
        let first = wait_for_row(&marker)?;
        oracle(&first)?;
        tty_write("   ")?;
        pause()?;
        let second = screen_check()?;
        oracle(&second)?;
        let digest = |s: &str| s.lines().find(|line| line.starts_with("console sha256: ")).unwrap_or("").to_string();
        require(digest(&first) != digest(&second), "moving the cursor across blank cells did not change the screen")
    })
}

fn screen_log_on_tty1() -> CaseResult {
    measured(|| {
        let sid = libbreenix::process::setsid().map_err(|e| format!("setsid: {e}"))?;
        wait_for_row(&format!("sys_setsid: process {} created new session", sid.raw())).map(drop)
    })
}

fn screen_log_quiet_in_session() -> CaseResult {
    measured(|| {
        libbreenix::process::setsid().map_err(|e| format!("setsid: {e}"))?;
        let fd = fs::open("/dev/console", fs::O_RDWR | O_NOCTTY).map_err(|e| format!("open console: {e}"))?;
        let acquired = unsafe { raw::syscall3(nr::IOCTL, fd.raw(), 0x540e, 1) };
        let _ = io::close(fd);
        Error::from_syscall(acquired as i64).map_err(|e| format!("acquire session: {e}"))?;
        pause()?;
        // A second process creates an INFO record after the session is held.
        let pid = match libbreenix::process::fork().map_err(|e| format!("fork: {e}"))? {
            libbreenix::process::ForkResult::Child => {
                let ok = libbreenix::process::setsid().is_ok();
                libbreenix::process::exit(if ok { 0 } else { 1 });
            }
            libbreenix::process::ForkResult::Parent(pid) => pid,
        };
        let mut status = 0;
        libbreenix::process::waitpid(pid.raw() as i32, &mut status, 0).map_err(|e| format!("waitpid: {e}"))?;
        require(status == 0, "INFO producer failed")?;
        // This is a real invalid memory request, refused before any mapping.
        let ret = unsafe { raw::syscall6(nr::MMAP, 0, 0, 3, 0x22, u64::MAX, 0) };
        require(ret as i64 == -22, "zero-length mmap did not fail with EINVAL")?;
        let text = wait_for_row("sys_mmap: length is 0")?;
        require(!text.lines().any(|line| line.starts_with("console row: ") &&
            line.contains(&format!("sys_setsid: process {} created new session", pid.raw()))),
            "INFO was drawn during the login session")
    })
}

struct DisplayOwner;
impl DisplayOwner {
    fn take() -> Verdict<Self> {
        libbreenix::graphics::take_over_display().map_err(|e| format!("take display: {e}"))?;
        Ok(Self)
    }
}
impl Drop for DisplayOwner {
    fn drop(&mut self) { let _ = libbreenix::graphics::give_back_display(); }
}

fn paint_owner() -> Verdict<libbreenix::graphics::FbInfo> {
    let info = libbreenix::graphics::fbinfo().map_err(|e| format!("fbinfo: {e}"))?;
    let (pixels, width) = libbreenix::graphics::fb_mmap_pane().map_err(|e| format!("fb_mmap: {e}"))?;
    require(width == info.width, "display owner did not map the whole screen")?;
    let bytes = (width * info.height * info.bytes_per_pixel) as usize;
    // The mapping holds the whole screen, using the reported pane stride.
    unsafe { core::ptr::write_bytes(pixels, 0x3c, bytes); }
    libbreenix::graphics::fb_flush().map_err(|e| format!("fb_flush: {e}"))?;
    Ok(info)
}

fn screen_display_taken_stops_drawing() -> CaseResult {
    measured(|| {
        let owner = DisplayOwner::take()?;
        let info = paint_owner()?;
        tty_write("\r\nconsole must retain this without drawing\r\n")?;
        pause()?;
        require(screen_check()?.contains("console check: a process has the display"), "console did not yield display")?;
        // Sample the text area, including cursor bands, from the display's
        // memory/backing rather than the owner's staging mapping.
        for y in (0..info.height as i32).step_by(13) {
            for x in (0..info.width as i32).step_by(29) {
                let pixel = libbreenix::graphics::fb_read_pixel(x, y).map_err(|e| format!("read pixel: {e}"))?;
                require(pixel == 0x3c3c3c, "console overwrote the display owner's pixels")?;
            }
        }
        drop(owner);
        Ok(())
    })
}

fn screen_display_released_redraws() -> CaseResult {
    measured(|| {
        let before = marker("before-take")?;
        tty_write(&format!("\r\n{before}\r\n"))?;
        wait_for_row(&before)?;
        let owner = DisplayOwner::take()?;
        paint_owner()?;
        let retained = marker("while-taken")?;
        tty_write(&format!("\r\n{retained}\r\n"))?;
        drop(owner);
        let text = wait_for_row(&retained)?;
        require(text.lines().any(|line| line.starts_with("console row: ") && line.contains(&before)),
            "release did not redraw the terminal's earlier text")?;
        oracle(&text)
    })
}

/// One function per case that has no check yet, each failing with the layer it needs.
macro_rules! needs {
    ($($name:ident => $layer:ident;)*) => {
        $(fn $name() -> CaseResult { missing(&$layer) })*
    };
}

needs! {
    emulation_sgr_colours => EMULATION;
    emulation_sgr_256_mapped => EMULATION;
    emulation_bold_reverse_underline => EMULATION;
    emulation_utf8_latin1 => EMULATION;
    emulation_utf8_replacement => EMULATION;
    emulation_box_drawing => EMULATION;
    emulation_scroll_region => EMULATION;
    emulation_insert_delete_line => EMULATION;
    emulation_insert_delete_char => EMULATION;
    emulation_save_restore_cursor => EMULATION;
    emulation_cursor_hidden => EMULATION;
    emulation_autowrap_off => EMULATION;
    emulation_tab_stops => EMULATION;
    emulation_alt_screen => EMULATION;
    emulation_full_reset => EMULATION;
    emulation_cursor_report => EMULATION;
    vt_devices_six_terminals => VT;
    vt_devices_tty1_is_console => VT;
    vt_devices_tty0_is_foreground => VT;
    vt_devices_dev_tty_is_ctty => VT;
    vt_devices_dev_tty_enxio => VT;
    vt_devices_termios_per_vt => VT;
    vt_devices_ctty_per_vt => VT;
    vt_devices_winsize_per_vt => VT;
    vt_devices_vcs_text => VT;
    vt_devices_vcsa_attributes => VT;
    vt_devices_background_output_retained => VT;
    vt_devices_background_process_runs => VT;
    vt_devices_background_read_waits => VT;
    vt_devices_switch_sends_no_signal => VT;
    vt_switch_activate => VT;
    vt_switch_activate_enxio => VT;
    vt_switch_waitactive => VT;
    vt_switch_waitactive_after_draw => VT;
    vt_switch_waitactive_eintr => VT;
    vt_switch_getstate => VT;
    vt_switch_openqry => VT;
    vt_switch_kdsetmode_graphics => VT;
    vt_switch_kd_owner_exit => VT;
    vt_switch_graphics_vt_refuses_switch => VT;
    vt_switch_vt_process_release => VT;
    vt_switch_vt_process_acquire => VT;
    vt_switch_vt_process_refuse => VT;
    vt_switch_vt_process_release_timeout => VT;
    vt_switch_vt_process_owner_exit => VT;
    vt_switch_vt_process_competing => VT;
    vt_switch_vt_process_cancel => VT;
    vt_switch_setmode_busy => VT;
    vt_switch_reldisp_errors => VT;
    vt_switch_eperm_unprivileged => VT;
    vt_switch_switch_readback => VT;
    vt_switch_switch_log_digest => VT;
    scrollback_history_kept => VT;
    scrollback_history_per_vt => VT;
    scrollback_survives_switch => VT;
    scrollback_view_holds_on_output => KEYS;
    scrollback_key_returns_live => KEYS;
    chvt_chvt_switches => USER;
    chvt_chvt_waits => USER;
    chvt_chvt_bad_number => USER;
    getty_prompt_each_vt => USER;
    getty_wrong_password_refused => USER;
    getty_login_on_vt => USER;
    getty_logout_reacquires => USER;
    getty_independent_sessions => USER;
    keys_alt_fn => KEYS;
    keys_ctrl_alt_fn => KEYS;
    keys_ctrl_alt_fn_graphics => KEYS;
    keys_alt_fn_graphics => KEYS;
    keys_alt_left_right_wrap => KEYS;
    keys_alt_f7_ignored => KEYS;
    keys_foreground_owns_input => KEYS;
    keys_ctrl_c_foreground_only => KEYS;
    keys_caps_per_vt => KEYS;
    keys_shift_pageup => KEYS;
}

fn main() {
    SUITE.run()
}
