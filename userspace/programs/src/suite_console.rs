//! Screen console & virtual terminals: the `console` effort's suite, measuring the contract in
//! `docs/console/contract.md`.
//!
//! One category per measure of an effort milestone (contract section 12): `screen`, `emulation`,
//! `vt-devices`, `vt-switch` and `scrollback`, `chvt` and `getty`, and `keys`.
//!
//! Each case checks its contract section against what the kernel does; a case whose layer is
//! absent fails, saying what the kernel answered, rather than skipping.

use libbreenix::error::Error;
use libbreenix::fs;
use libbreenix::io;
use libbreenix::suite::{case, category, suite, CaseResult, Suite};
use libbreenix::syscall::{nr, raw};

#[path = "suite_console_vt.rs"]
mod vt_cases;
use vt_cases::*;

static SUITE: Suite = suite("console", "Screen console & virtual terminals", &[
    category("screen", "Console on the screen", &[
        case("tty1-on-screen", "Text written to tty1 is drawn on the screen", screen_tty1_on_screen),
        case("readback-identical", "The displayed pixels read back identical to the console's shadow", screen_readback_identical),
        case("scanout-bound", "The read-back is of the resource bound to the scanout, after a completed flush", screen_scanout_bound),
        case("font-oracle", "Drawn cells match the glyphs the font oracle expects", screen_font_oracle),
        case("size-class", "The font size and grid follow the size rule for the display", screen_size_class),
        case("scanout-grid", "The scanned-out screen shows the foreground terminal's grid at the size rule's scale, and no diagnostic-only kernel line", screen_scanout_grid),
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
        case("root-login-on-vt", "Root logs in with its own password on a numbered terminal under the shipped ROOT_LOGIN no", getty_root_login_on_vt),
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
        case("report-joins-typed-line", "A cursor report joins the partly typed canonical line, after its keys", keys_report_joins_typed_line),
    ]),
]).defer_display();

/// `VT_GETSTATE`, from the published `linux/vt.h`: writes a 6-byte `struct vt_stat`.
const VT_GETSTATE: u64 = 0x5603;
/// `O_NOCTTY`, the same on x86-64 and ARM64, so opening a terminal does not make it the case's controlling terminal.
const O_NOCTTY: u32 = 0o400;

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

/// The check's `console display:` line: the display's size, the font size, the cell size
/// and the grid.
struct Shown {
    display: (usize, usize),
    size: usize,
    cell: (usize, usize),
    grid: (usize, usize),
}

fn shown_display(text: &str) -> Verdict<Shown> {
    let line = text.lines().find_map(|line| line.strip_prefix("console display: ")).ok_or("missing display")?;
    let display = pair(line.split_whitespace().next().ok_or("missing dimensions")?)?;
    let (_, rest) = line.split_once(", font ").ok_or("missing font")?;
    let (size, rest) = rest.split_once(" (").ok_or("missing cell")?;
    let (cell, grid) = rest.split_once(" pixel cells), grid ").ok_or("missing grid")?;
    Ok(Shown { display, size: size.parse().map_err(|e| format!("font size: {e}"))?, cell: pair(cell)?, grid: pair(grid)? })
}

/// The size rule (contract section 2) for a display: the font size, its cell and the grid.
fn size_rule((width, height): (usize, usize)) -> (usize, (usize, usize), (usize, usize)) {
    let fonts = [(16, 7, 18), (20, 9, 22), (24, 11, 26), (32, 14, 34)];
    let &(size, cw, ch) = fonts.iter().rev().find(|&&(_, cw, ch)| width / cw >= 128 && height / ch >= 36)
        .unwrap_or(&fonts[0]);
    (size, (cw, ch), ((width / cw).min(255), (height / ch).min(255)))
}

fn screen_size_class() -> CaseResult {
    measured(|| {
        let text = screen_check()?;
        identical(&text)?;
        let shown = shown_display(&text)?;
        let (size, cell, grid) = size_rule(shown.display);
        require(shown.size == size && shown.cell == cell && shown.grid == grid,
            &format!("font {} cell {:?} grid {:?}, the size rule gives font {size} cell {cell:?} grid {grid:?}",
                shown.size, shown.cell, shown.grid))
    })
}

/// Kernel diagnostic records: census and trace lines written for gates and debugging, which
/// go to serial and the kernel log, not to a screen (contract section 1.3).
const DIAGNOSTICS: [&str; 6] = [
    "[syscall] exit(", "[TTBR0_ASID_CENSUS:", "[PT_ROOT_CUSTODY:", "[TOMBSTONE_CENSUS:",
    "manager.create_process_with_argv", "[spawn] ",
];

/// What the GPU core shows, when it owns the display: the current mode's size and the
/// resource bound to scanout 0, from `MODES`.
fn gpu_display() -> Verdict<Option<((usize, usize), u64)>> {
    use libbreenix::gpu::{self, abi, abi::Plain};
    let (r, caps) = gpu::caps();
    if r != 0 || caps.status != abi::status::ADMITTED {
        return Ok(None);
    }
    let mut modes = abi::Modes::ZERO;
    let r = gpu::call_with(abi::op::MODES, &mut modes);
    require(r == 0, &format!("MODES returned {r}"))?;
    require(modes.binding_known == 1, "the GPU core does not know what scanout 0 shows")?;
    let mode = modes.modes.get(modes.current as usize).ok_or("MODES names no current mode")?;
    Ok(Some(((mode.width as usize, mode.height as usize), modes.scanout_resource)))
}

/// `TIOCGWINSZ` on `/dev/tty1`: rows, columns, then the pixel width and height.
fn tty1_winsize() -> Verdict<[u16; 4]> {
    const TIOCGWINSZ: u64 = 0x5413;
    let fd = fs::open("/dev/tty1", fs::O_RDONLY | O_NOCTTY).map_err(|e| format!("open tty1: {e}"))?;
    let mut size = [0u16; 4];
    let ret = unsafe { raw::syscall3(nr::IOCTL, fd.raw(), TIOCGWINSZ, size.as_mut_ptr() as u64) };
    let _ = io::close(fd);
    Error::from_syscall(ret as i64).map_err(|e| format!("TIOCGWINSZ: {e}"))?;
    Ok(size)
}

/// Read back what the display scans out: the console's frame must be the one bound to
/// scanout 0 (on the GPU core, the resource `MODES` names, transferred back from the
/// device), drawn with the foreground terminal's grid at the size rule's font and logo scale
/// for that display, and with no kernel diagnostic line on it though the kernel logged one.
fn screen_scanout_grid() -> CaseResult {
    measured(|| {
        // Diagnostics to look for: a child's exit (ARM64 logs it and the ASID census) and a
        // read of the trace counters (both architectures log the root and tombstone census).
        let child = match libbreenix::process::fork().map_err(|e| format!("fork: {e}"))? {
            libbreenix::process::ForkResult::Child => libbreenix::process::exit(0),
            libbreenix::process::ForkResult::Parent(pid) => pid,
        };
        let mut status = 0;
        libbreenix::process::waitpid(child.raw() as i32, &mut status, 0).map_err(|e| format!("waitpid: {e}"))?;
        std::fs::read_to_string("/proc/trace/counters").map_err(|e| format!("trace counters: {e}"))?;
        let marker = marker("scanout")?;
        tty_write(&format!("\r\n{marker}\r\n"))?;
        let text = wait_for_row(&marker)?;
        oracle(&text)?;
        require(text.contains("console foreground: tty1\n"), &text)?;

        let shown = shown_display(&text)?;
        let source = text.lines().find_map(|line| line.strip_prefix("console scanout: ")).ok_or("missing scanout source")?;
        match gpu_display()? {
            Some((display, resource)) => {
                require(shown.display == display,
                    &format!("the console draws {:?}, the GPU core shows {display:?}", shown.display))?;
                require(source.contains(&format!(" of resource {resource} bound to scanout 0"))
                    && source.ends_with("transferred back from the device"),
                    &format!("the GPU core shows resource {resource}, the console read {source}"))?;
            }
            None => require(source.starts_with("scanout memory at ")
                || (source.starts_with("backing after flush ") && source.contains("bound to scanout 0")), source)?,
        }
        let (size, cell, grid) = size_rule(shown.display);
        require(shown.size == size && shown.cell == cell && shown.grid == grid,
            &format!("font {} cell {:?} grid {:?}, the size rule gives font {size} cell {cell:?} grid {grid:?} for {:?}",
                shown.size, shown.cell, shown.grid, shown.display))?;
        let logo = text.lines().find_map(|line| line.strip_prefix("console logo scale: ")).ok_or("missing logo scale")?;
        let k = match size { 32 => 4, 24 => 3, _ => 2 };
        require(logo == k.to_string(), &format!("logo scale {logo}, the splash's rule gives {k} for font {size}"))?;
        let winsize = tty1_winsize()?;
        let wanted = [grid.1 as u16, grid.0 as u16, (grid.0 * cell.0) as u16, (grid.1 * cell.1) as u16];
        require(winsize == wanted, &format!("tty1 reports {winsize:?}, the screen shows {wanted:?}"))?;
        let header = std::fs::read("/dev/vcsa1").map_err(|e| format!("read vcsa1: {e}"))?;
        require(header.len() >= 2 && (usize::from(header[0]), usize::from(header[1])) == (grid.1, grid.0),
            "vcsa1's header does not give the grid on the screen")?;

        let log = std::fs::read_to_string("/proc/kmsg").map_err(|e| format!("kmsg: {e}"))?;
        #[cfg(target_arch = "aarch64")]
        let logged = format!("[syscall] exit(0) pid={} ", child.raw());
        #[cfg(target_arch = "x86_64")]
        let logged = "[TOMBSTONE_CENSUS:".to_string();
        require(log.contains(&logged), &format!("the kernel log has no {logged} line to keep off the screen"))?;
        for row in text.lines().filter_map(|line| line.strip_prefix("console row: ")) {
            if let Some(found) = DIAGNOSTICS.iter().find(|&&diagnostic| row.contains(diagnostic)) {
                return Err(format!("a diagnostic line ({found}) is on the screen: {row}"));
            }
        }
        Ok(())
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


// Emulation cases hold a console session, so routine INFO records cannot move
// their cursor. They inspect the displayed pixels and the render-thread check.
struct Terminal {
    fd: libbreenix::types::Fd,
    original: libbreenix::termios::Termios,
    cols: usize,
    cw: usize,
    ch: usize,
}

impl Terminal {
    fn open() -> Verdict<Self> {
        use libbreenix::termios as t;
        libbreenix::process::setsid().map_err(|e| format!("setsid: {e}"))?;
        let fd = fs::open("/dev/console", fs::O_RDWR | O_NOCTTY | 0o4000)
            .map_err(|e| format!("open console: {e}"))?;
        let acquired = unsafe { raw::syscall3(nr::IOCTL, fd.raw(), t::request::TIOCSCTTY, 1) };
        Error::from_syscall(acquired as i64).map_err(|e| format!("console session: {e}"))?;
        let mut original = t::Termios::default();
        t::tcgetattr(fd, &mut original).map_err(|e| format!("tcgetattr: {e}"))?;
        let mut raw = original;
        raw.c_lflag &= !(t::lflag::ICANON | t::lflag::ECHO);
        raw.c_oflag = 0;
        raw.c_cc[t::cc::VMIN] = 0;
        raw.c_cc[t::cc::VTIME] = 0;
        t::tcsetattr(fd, t::TCSANOW, &raw).map_err(|e| format!("tcsetattr: {e}"))?;
        let check = screen_check()?;
        let line = check.lines().find_map(|l| l.strip_prefix("console display: ")).ok_or("missing display")?;
        let (_, rest) = line.split_once(" (").ok_or("missing cell size")?;
        let (cell, grid) = rest.split_once(" pixel cells), grid ").ok_or("missing grid size")?;
        let (cw, ch) = pair(cell)?;
        let (cols, _) = pair(grid)?;
        let terminal = Self { fd, original, cols, cw, ch };
        terminal.write(b"\x1bc")?;
        Ok(terminal)
    }

    fn write(&self, mut bytes: &[u8]) -> Verdict {
        while !bytes.is_empty() {
            let n = io::write(self.fd, bytes).map_err(|e| format!("console write: {e}"))?;
            require(n > 0, "console write made no progress")?;
            bytes = &bytes[n..];
        }
        Ok(())
    }

    fn check(&self) -> Verdict<String> {
        pause()?;
        let check = screen_check()?;
        identical(&check)?;
        require(check.lines().any(|l| l.starts_with("console oracle: ") && l.ends_with(" mismatches=0")), &check)?;
        Ok(check)
    }

    fn rows(&self, expected: &[&str]) -> Verdict {
        let check = self.check()?;
        let rows: Vec<_> = check.lines().filter_map(|l| l.strip_prefix("console row: ")).collect();
        require(rows == expected, &format!("rows {rows:?}, expected {expected:?}"))
    }

    fn pixel(&self, col: usize, row: usize, x: usize, y: usize, rgb: u32) -> Verdict {
        let actual = libbreenix::graphics::fb_read_pixel((col * self.cw + x) as i32, (row * self.ch + y) as i32)
            .map_err(|e| format!("pixel read: {e}"))?;
        require(actual == rgb, &format!("cell {row},{col} pixel {x},{y}: {actual:06x}, expected {rgb:06x}"))
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.write(b"\x1bc");
        let _ = libbreenix::termios::tcsetattr(self.fd, libbreenix::termios::TCSANOW, &self.original);
        let _ = io::close(self.fd);
    }
}

const COLOURS: [u32; 16] = [
    0x0f1423, 0xcd4646, 0x46c878, 0xd7b446, 0x4678d7, 0xaf5fc8, 0x3cafb9, 0xcdd7eb,
    0x5a647d, 0xf56e6e, 0x78e6a0, 0xf5d76e, 0x78a5f5, 0xd78cf0, 0x6ed7e1, 0xf5f8ff,
];

fn emulation_sgr_colours() -> CaseResult {
    measured(|| {
        let t = Terminal::open()?;
        for i in 0..16 {
            let bg = if i < 8 { 40 + i } else { 100 + i - 8 };
            let fg = if i < 8 { 30 + i } else { 90 + i - 8 };
            t.write(format!("\x1b[{bg}m \x1b[0;{fg};4m ").as_bytes())?;
        }
        t.write(b"\x1b[0mX\x1b[?25l")?;
        t.check()?;
        // Taking ownership quiesces drawing and preserves the completed frame.
        let _owner = DisplayOwner::take()?;
        for (i, colour) in COLOURS.iter().enumerate() {
            t.pixel(2*i, 0, 0, 0, *colour)?;
            t.pixel(2*i+1, 0, 0, t.ch-2, *colour)?;
        }
        Ok(())
    })
}

fn emulation_sgr_256_mapped() -> CaseResult {
    measured(|| {
        let t = Terminal::open()?;
        t.write(b"\x1b[48;5;196m \x1b[48;5;46m \x1b[48;5;232m \x1b[48;2;120;165;245m \x1b[48;5;15m \x1b[0mX\x1b[?25l")?;
        t.check()?;
        // Taking ownership quiesces drawing and preserves the completed frame.
        let _owner = DisplayOwner::take()?;
        // Nearest squared RGB distance, from the contract palette.
        for (col, index) in [1, 2, 0, 12, 15].into_iter().enumerate() {
            t.pixel(col, 0, 0, 0, COLOURS[index])?;
        }
        Ok(())
    })
}

fn emulation_bold_reverse_underline() -> CaseResult {
    measured(|| {
        let t = Terminal::open()?;
        t.write(b"\x1b[?25lW")?;
        let regular = t.check()?;
        t.write(b"\x1b[H\x1b[1mW")?;
        let bold = t.check()?;
        let digest = |s: &str| s.lines().find(|l| l.starts_with("console sha256:")).unwrap_or("").to_string();
        require(digest(&regular) != digest(&bold), "bold did not change the glyph")?;
        t.write(b"\x1b[H\x1b[0;31;44;7m \x1b[0;32;4m \x1b[0;33;45;8mW")?;
        t.check()?;
        // Taking ownership quiesces drawing and preserves the completed frame.
        let _owner = DisplayOwner::take()?;
        t.pixel(0, 0, 0, 0, COLOURS[1])?;
        t.pixel(1, 0, 0, t.ch-2, COLOURS[2])?;
        for y in 0..t.ch { for x in 0..t.cw { t.pixel(2, 0, x, y, COLOURS[5])?; } }
        Ok(())
    })
}

fn emulation_utf8_latin1() -> CaseResult {
    measured(|| {
        let t = Terminal::open()?;
        t.write("Àéÿ ĀŁſ".as_bytes())?;
        t.rows(&["Àéÿ ĀŁſ"])
    })
}

fn emulation_utf8_replacement() -> CaseResult {
    measured(|| {
        let t = Terminal::open()?;
        t.write(b"a\xe2\x82b\xc0\xaf")?;
        t.write("🙂!".as_bytes())?;
        t.rows(&["a?b???!"])
    })
}

fn emulation_box_drawing() -> CaseResult {
    measured(|| {
        let t = Terminal::open()?;
        let repertoire: String = (0x2500..=0x257f).map(|c| char::from_u32(c).unwrap()).collect();
        t.write(repertoire.as_bytes())?;
        t.rows(&[&repertoire])?;
        t.write("\x1b[H\x1b[?25l┌─┐\r\n│ │\r\n└─┘".as_bytes())?;
        t.check()?;
        // Taking ownership quiesces drawing and preserves the completed frame.
        let _owner = DisplayOwner::take()?;
        // Shared horizontal and vertical seams must meet at cell boundaries.
        for col in 0..3 {
            t.pixel(col, 0, if col == 0 { t.cw-1 } else { 0 }, t.ch/2, COLOURS[7])?;
        }
        t.pixel(0, 1, t.cw/2, 0, COLOURS[7])?;
        t.pixel(0, 1, t.cw/2, t.ch-1, COLOURS[7])
    })
}

fn emulation_scroll_region() -> CaseResult {
    measured(|| {
        let t = Terminal::open()?;
        t.write(b"A\r\nB\r\nC\r\nD\r\nE\x1b[2;4r\x1b[4;1H\nX")?;
        t.rows(&["A", "C", "D", "X", "E"])?;
        t.write(b"\x1b[T\x1b[2;1HY\x1bMZ")?;
        t.rows(&["A", " Z", "Y", "C", "E"])
    })
}

fn emulation_insert_delete_line() -> CaseResult {
    measured(|| {
        let t = Terminal::open()?;
        t.write(b"A\r\nB\r\nC\r\nD\r\nE\x1b[2;4r\x1b[3;1H\x1b[LX")?;
        t.rows(&["A", "B", "X", "C", "E"])?;
        t.write(b"\x1b[2;1H\x1b[MY")?;
        t.rows(&["A", "Y", "C", "E"])
    })
}

fn emulation_insert_delete_char() -> CaseResult {
    measured(|| {
        let t = Terminal::open()?;
        t.write(b"abcdef\x1b[3G\x1b[2@XY\x1b[2P\x1b[1G\x1b[2X")?;
        t.rows(&["  XYef"])
    })
}

fn emulation_save_restore_cursor() -> CaseResult {
    measured(|| {
        let t = Terminal::open()?;
        t.write(b"\x1b[31mA\x1b7\x1b[3;5H\x1b[32mB\x1b8\x1b[4m \x1b[s\x1b[H\x1b[uC\x1b[?25l")?;
        t.rows(&["A C", "    B"])?;
        let _owner = DisplayOwner::take()?;
        t.pixel(1, 0, 0, t.ch-2, COLOURS[1])
    })
}

fn emulation_cursor_hidden() -> CaseResult {
    measured(|| {
        let t = Terminal::open()?;
        t.write(b"X\x1b[?25l")?;
        t.check()?;
        {
            let _owner = DisplayOwner::take()?;
            t.pixel(1, 0, 0, t.ch-1, COLOURS[0])?;
        }
        t.write(b"\x1b[?25h")?;
        t.check()?;
        let _owner = DisplayOwner::take()?;
        t.pixel(1, 0, 0, t.ch-1, COLOURS[7])
    })
}

fn emulation_autowrap_off() -> CaseResult {
    measured(|| {
        let t = Terminal::open()?;
        t.write(format!("\x1b[?7l\x1b[{}GXYZ", t.cols).as_bytes())?;
        t.rows(&[&format!("{}Z", " ".repeat(t.cols-1))])
    })
}

fn emulation_tab_stops() -> CaseResult {
    measured(|| {
        let t = Terminal::open()?;
        t.write(b"A\tB\x1b[3g\x1b[5G\x1bH\x1b[1G\tC\x1b[5G\x1b[g\x1b[1G\tD")?;
        t.rows(&[&format!("A   C   B{}D", " ".repeat(t.cols-10))])
    })
}

fn emulation_alt_screen() -> CaseResult {
    measured(|| {
        let t = Terminal::open()?;
        t.write(b"main\x1b[31m\x1b[?1049h")?;
        t.rows(&[])?;
        t.write(b"other\x1b[?1049h")?;
        t.rows(&["other"])?;
        t.write(b"\x1b[?1049l\x1b[4m \x1b[?25l")?;
        t.rows(&["main"])?;
        let _owner = DisplayOwner::take()?;
        t.pixel(4, 0, 0, t.ch-2, COLOURS[1])
    })
}

fn emulation_full_reset() -> CaseResult {
    measured(|| {
        let t = Terminal::open()?;
        t.write(b"old\x1b[3g\x1b[31;44;1m\x1b[?7l\x1b[?25l\x1b[2;4r\x1b[?1049hother\x1bcA\tB")?;
        t.rows(&["A       B"])?;
        {
            let _owner = DisplayOwner::take()?;
            t.pixel(9, 0, 0, t.ch-1, COLOURS[7])?;
        }
        t.write(format!("\x1b[{}Gxy\x1b[?1049l", t.cols).as_bytes())?;
        t.rows(&[&format!("A       B{}x", " ".repeat(t.cols-10)), "y"])
    })
}

fn emulation_cursor_report() -> CaseResult {
    measured(|| {
        let t = Terminal::open()?;
        libbreenix::termios::tcflush(t.fd, libbreenix::termios::TCIFLUSH).map_err(|e| e.to_string())?;
        t.write(b"\x1b[4;9H\x1b[6n\x1b[5n")?;
        let mut reply = Vec::new();
        for _ in 0..20 {
            let mut bytes = [0; 32];
            match io::read(t.fd, &mut bytes) {
                Ok(n) => reply.extend_from_slice(&bytes[..n]),
                Err(Error::Os(libbreenix::errno::Errno::EAGAIN)) => {},
                Err(e) => return Err(format!("reply read: {e}")),
            }
            if reply.len() >= b"\x1b[4;9R\x1b[0n".len() { break; }
            pause()?;
        }
        require(reply == b"\x1b[4;9R\x1b[0n", &format!("reply {reply:?}"))
    })
}

// Userspace VT cases use the contracted ioctls and screen devices. Login input
// comes through the host keyboard, including when serial remains attached to tty1.
fn vt_ioctl(fd: libbreenix::types::Fd, request: u64, arg: u64) -> Verdict<u64> {
    let ret = unsafe { raw::syscall3(nr::IOCTL, fd.raw(), request, arg) };
    Error::from_syscall(ret as i64).map_err(|e| format!("ioctl {request:x}: {e}"))
}

struct VtFd(libbreenix::types::Fd);
impl VtFd {
    fn open(number: usize) -> Verdict<Self> {
        fs::open(&format!("/dev/tty{number}"), fs::O_RDWR | O_NOCTTY)
            .map(Self).map_err(|e| format!("tty{number}: {e}"))
    }
}
impl Drop for VtFd { fn drop(&mut self) { let _ = io::close(self.0); } }

fn activate(number: usize) -> Verdict {
    let fd = VtFd::open(0)?;
    vt_ioctl(fd.0, 0x5606, number as u64)?;
    vt_ioctl(fd.0, 0x5607, number as u64)?;
    Ok(())
}

fn foreground() -> Verdict<usize> {
    let fd = VtFd::open(0)?;
    let mut state = [0u16; 3];
    vt_ioctl(fd.0, VT_GETSTATE, state.as_mut_ptr() as u64)?;
    Ok(state[0] as usize)
}

struct VtChild(i32, bool);
impl VtChild {
    fn spawn(path: &str, args: &[&str]) -> Verdict<Self> {
        let path = std::ffi::CString::new(path).map_err(|e| e.to_string())?;
        let args: Vec<_> = args.iter().map(|arg| std::ffi::CString::new(*arg).unwrap()).collect();
        let mut argv: Vec<_> = args.iter().map(|arg| arg.as_ptr().cast::<u8>()).collect();
        argv.push(core::ptr::null());
        libbreenix::process::spawnv(path.as_bytes_with_nul(), argv.as_ptr())
            .map(|pid| Self(pid.raw() as i32, false)).map_err(|e| format!("spawn: {e}"))
    }
    fn poll(&mut self) -> Verdict<Option<i32>> {
        let mut status = 0;
        let pid = libbreenix::process::waitpid(self.0, &mut status, libbreenix::process::WNOHANG)
            .map_err(|e| format!("waitpid: {e}"))?;
        if pid.raw() == 0 { return Ok(None); }
        self.1 = true;
        Ok(Some(status))
    }
    fn wait(&mut self) -> Verdict<i32> {
        while libbreenix::suite::case_ms_left() > 300 {
            if let Some(status) = self.poll()? { return Ok(status); }
            libbreenix::time::sleep_ms(20).map_err(|e| e.to_string())?;
        }
        Err("child did not finish before the case deadline".into())
    }
}
impl Drop for VtChild {
    fn drop(&mut self) {
        if !self.1 {
            let _ = libbreenix::signal::kill(self.0, libbreenix::signal::SIGKILL);
            let mut status = 0;
            let _ = libbreenix::process::waitpid(self.0, &mut status, 0);
        }
    }
}

fn chvt_chvt_switches() -> CaseResult {
    measured(|| {
        let tty3 = VtFd::open(3)?;
        io::write(tty3.0, b"\x1bcchvt-switch-witness\r\n").map_err(|e| e.to_string())?;
        let pid = match libbreenix::process::fork().map_err(|e| e.to_string())? {
            libbreenix::process::ForkResult::Child => {
                let result = (|| -> Verdict {
                    libbreenix::process::setsid().map_err(|e| e.to_string())?;
                    vt_ioctl(tty3.0, 0x540E, 0)?; // TIOCSCTTY before dropping root.
                    #[cfg(target_arch = "x86_64")]
                    const SETUID: u64 = 105;
                    #[cfg(target_arch = "aarch64")]
                    const SETUID: u64 = 146;
                    require(unsafe { raw::syscall1(SETUID, 1000) } as i64 == 0, "setuid(1000)")?;
                    let mut command = VtChild::spawn("/usr/bin/chvt", &["chvt", "3"])?;
                    require(command.wait()? == 0, "non-root chvt 3 failed")
                })();
                if let Err(reason) = &result { eprintln!("chvt credential probe: {reason}"); }
                libbreenix::process::exit(if result.is_ok() { 0 } else { 1 });
            }
            libbreenix::process::ForkResult::Parent(pid) => pid,
        };
        let mut child = VtChild(pid.raw() as i32, false);
        require(child.wait()? == 0, "chvt with a non-root controlling VT failed")?;
        require(foreground()? == 3, "chvt did not select tty3")?;
        let check = screen_check()?;
        require(check.lines().any(|l| l == "console foreground: tty3"), "read-back is not tty3")?;
        require(check.lines().any(|l| l == "console row: chvt-switch-witness"), "tty3 witness missing")?;
        identical(&check)?;
        activate(1)
    })
}

fn chvt_chvt_bad_number() -> CaseResult {
    measured(|| {
        let before = foreground()?;
        for number in ["0", "7", "18446744073709551615", "abc"] {
            let mut child = VtChild::spawn("/usr/bin/chvt", &["chvt", number])?;
            require(child.wait()? == 256, "invalid chvt did not exit 1")?;
            require(foreground()? == before, "invalid chvt changed the foreground")?;
        }
        Ok(())
    })
}

fn chvt_chvt_waits() -> CaseResult {
    measured(|| {
        activate(2)?;
        let (ready_r, ready_w) = io::pipe().map_err(|e| e.to_string())?;
        let (release_r, release_w) = io::pipe().map_err(|e| e.to_string())?;
        let owner = match libbreenix::process::fork().map_err(|e| e.to_string())? {
            libbreenix::process::ForkResult::Child => {
                let result = (|| -> Verdict {
                    let fd = VtFd::open(2)?;
                    let ignored = libbreenix::signal::Sigaction {
                        handler: libbreenix::signal::SIG_IGN, ..Default::default()
                    };
                    for sig in [10, 12] {
                        libbreenix::signal::sigaction(sig, Some(&ignored), None).map_err(|e| e.to_string())?;
                    }
                    // vt_mode: mode, waitv, relsig, acqsig, frsig (8 bytes).
                    let mode: [u16; 4] = [1, 10, 12, 0];
                    vt_ioctl(fd.0, 0x5602, mode.as_ptr() as u64)?;
                    io::write(ready_w, b"R").map_err(|e| e.to_string())?;
                    let mut byte = [0];
                    io::read(release_r, &mut byte).map_err(|e| e.to_string())?;
                    vt_ioctl(fd.0, 0x5605, 1)?;
                    Ok(())
                })();
                libbreenix::process::exit(if result.is_ok() { 0 } else { 1 });
            }
            libbreenix::process::ForkResult::Parent(pid) => VtChild(pid.raw() as i32, false),
        };
        let mut owner = owner;
        io::close(ready_w).map_err(|e| e.to_string())?;
        io::close(release_r).map_err(|e| e.to_string())?;
        let mut byte = [0];
        require(io::read(ready_r, &mut byte).map_err(|e| e.to_string())? == 1 && byte == *b"R", "VT owner failed")?;
        let mut child = VtChild::spawn("/usr/bin/chvt", &["chvt", "3"])?;
        pause()?;
        require(child.poll()?.is_none(), "chvt returned before VT_PROCESS released")?;
        require(foreground()? == 2, "VT_PROCESS did not hold tty2")?;
        io::write(release_w, b"R").map_err(|e| e.to_string())?;
        io::close(release_w).map_err(|e| e.to_string())?;
        io::close(ready_r).map_err(|e| e.to_string())?;
        require(child.wait()? == 0, "chvt failed after release")?;
        require(owner.wait()? == 0, "VT owner release failed")?;
        require(foreground()? == 3, "chvt returned before tty3 was active")?;
        let check = screen_check()?;
        require(check.lines().any(|l| l == "console foreground: tty3"), "read-back is not tty3")?;
        identical(&check)?;
        activate(1)
    })
}

// C4 accepts the staged root account. Give it fresh random credentials and a
// session-checking shell for this case, restoring both files on the private disk.
struct LoginCredentials { password: String, hash: String }
static LOGIN_CREDENTIALS: std::sync::OnceLock<Verdict<LoginCredentials>> = std::sync::OnceLock::new();

fn login_credentials() -> Verdict<LoginCredentials> {
    // S41: this throwaway test password is not a secret. It must also work
    // on platforms whose entropy pool correctly remains unready.
    let mut random = [0; 16];
    let mut at = 0;
    while at < random.len() {
        let n = unsafe {
            raw::syscall3(nr::GETRANDOM, random[at..].as_mut_ptr() as u64,
                (random.len() - at) as u64, libbreenix::syscall::GRND_INSECURE as u64)
        } as i64;
        require(n >= 0, &format!("getrandom: {n}"))?;
        let n = n as usize;
        require(n > 0, "getrandom made no progress")?;
        at += n;
    }
    // Encode the 64 random bits in 13 unmodified keyboard chords. The two-login
    // cases have named allowances for host injection, verification and respawn.
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut bits = u64::from_le_bytes(random[..8].try_into().map_err(|_| "random word length")?);
    let mut password = String::new();
    for _ in 0..13 {
        password.push(ALPHABET[(bits % 36) as usize] as char);
        bits /= 36;
    }
    let salt: String = random[8..].iter().map(|b| format!("{b:02x}")).collect();
    let mut hash = [0; libbreenix::crypto::sha512_crypt::MAX_OUTPUT];
    let n = libbreenix::crypto::sha512_crypt::sha512_crypt(password.as_bytes(), &format!("$6${salt}"), &mut hash)
        .map_err(String::from)?;
    Ok(LoginCredentials { password, hash: std::str::from_utf8(&hash[..n]).map_err(|e| e.to_string())?.into() })
}

/// The getty cases' account: an ordinary user whose shell is the login-shell
/// oracle, listed in /etc/shells.
const LOGIN_NAME: &str = "vtuser";
struct LoginAccount { passwd: Vec<u8>, shadow: Option<Vec<u8>>, shells: Vec<u8>, password: String }
impl LoginAccount {
    fn create() -> Verdict<Self> {
        Self::stage(false)
    }
    /// Root with the run's password and the login-shell oracle as its shell,
    /// under the image's shipped login.defs (root logs in on the local
    /// console and virtual terminals, ruling S14).
    fn root() -> Verdict<Self> {
        Self::stage(true)
    }
    fn stage(root: bool) -> Verdict<Self> {
        let credentials = LOGIN_CREDENTIALS.get().ok_or("run credentials were not prepared")?
            .as_ref().map_err(Clone::clone)?;
        let passwd = std::fs::read("/etc/passwd").map_err(|e| e.to_string())?;
        let shadow = match std::fs::read("/etc/shadow") {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(format!("read shadow: {error}")),
        };
        let shells = std::fs::read("/etc/shells").map_err(|e| format!("read shells: {e}"))?;
        let account = Self { passwd, shadow, shells, password: credentials.password.clone() };
        let (passwd, shadow) = if root {
            ("root:x:0:0:root:/root:/usr/bin/console-login-shell\n".to_string(), format!("root:{}:0:0:99999:7:::\n", credentials.hash))
        } else {
            (format!("root:x:0:0:root:/root:/bin/bsh\n{LOGIN_NAME}:x:2010:2010:Console suite:/:/usr/bin/console-login-shell\n"),
                format!("root:!:::::::\n{LOGIN_NAME}:{}:0:0:99999:7:::\n", credentials.hash))
        };
        std::fs::write("/etc/passwd", passwd).map_err(|e| e.to_string())?;
        std::fs::write("/etc/shadow", shadow).map_err(|e| e.to_string())?;
        std::fs::write("/etc/shells", b"/bin/bsh\n/usr/bin/console-login-shell\n").map_err(|e| e.to_string())?;
        Ok(account)
    }
    fn restore(&self) -> Verdict {
        match &self.shadow {
            Some(bytes) => std::fs::write("/etc/shadow", bytes).map_err(|e| e.to_string())?,
            None => match std::fs::remove_file("/etc/shadow") {
                Ok(()) => {},
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                Err(error) => return Err(format!("remove test shadow: {error}")),
            },
        }
        std::fs::write("/etc/passwd", &self.passwd).map_err(|e| e.to_string())?;
        std::fs::write("/etc/shells", &self.shells).map_err(|e| e.to_string())?;
        let shadow_restored = match (std::fs::read("/etc/shadow"), &self.shadow) {
            (Ok(actual), Some(expected)) => actual == *expected,
            (Err(error), None) => error.kind() == std::io::ErrorKind::NotFound,
            _ => false,
        };
        require(shadow_restored && std::fs::read("/etc/passwd").map_err(|e| e.to_string())? == self.passwd
            && std::fs::read("/etc/shells").map_err(|e| e.to_string())? == self.shells,
            "account files were not restored")
    }
}
impl Drop for LoginAccount { fn drop(&mut self) { let _ = self.restore(); } }

fn start_gettys() -> Verdict<VtChild> {
    // The suite itself boots on tty1. Release that inherited ownership before
    // starting init's console target, as a real PID 1 does before its gettys.
    libbreenix::process::setsid().map_err(|e| e.to_string())?;
    let ignored = libbreenix::signal::Sigaction {
        handler: libbreenix::signal::SIG_IGN, ..Default::default()
    };
    libbreenix::signal::sigaction(libbreenix::signal::SIGHUP, Some(&ignored), None).map_err(|e| e.to_string())?;
    let console = VtFd::open(1)?;
    vt_ioctl(console.0, 0x540e, 1)?;
    vt_ioctl(console.0, 0x5422, 0)?;
    drop(console);
    for number in 1..=6 {
        let fd = VtFd::open(number)?;
        io::write(fd.0, b"\x1bc").map_err(|e| e.to_string())?;
    }
    VtChild::spawn("/sbin/init", &["init", "console"])
}

fn vt_text(number: usize) -> Verdict<String> {
    let bytes = std::fs::read(format!("/dev/vcsa{number}")).map_err(|e| e.to_string())?;
    require(bytes.len() >= 4 && bytes[0] != 0 && bytes[1] != 0, "invalid vcsa header")?;
    require(bytes.len() == 4 + 2 * bytes[0] as usize * bytes[1] as usize, "short vcsa screen")?;
    let mut text = String::new();
    for row in bytes[4..].chunks(2 * bytes[1] as usize) {
        for cell in row.chunks_exact(2) { text.push(cell[0] as char); }
        text.push('\n');
    }
    Ok(text)
}

fn wait_vt(number: usize, needle: &str) -> Verdict<String> {
    while libbreenix::suite::case_ms_left() > 500 {
        let text = vt_text(number)?;
        if text.contains(needle) { return Ok(text); }
        libbreenix::time::sleep_ms(20).map_err(|e| e.to_string())?;
    }
    Err(format!("tty{number} did not show {needle}"))
}

fn type_line(text: &str) -> Verdict {
    require(text.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()), "unsupported keyboard text")?;
    let mut keys: Vec<_> = text.chars().map(|c| c.to_string()).collect();
    keys.push("enter".into());
    libbreenix::suite::hostkeys(&keys.join(",")).map(|_| ())
}

// The terminal's last three non-blank rows. Getty echoes the login name and
// login reports its own failure there, so a failed login's verdict carries them.
fn screen_ends(number: usize) -> String {
    vt_text(number).map(|text| {
        let rows: Vec<_> = text.lines().map(str::trim_end).filter(|row| !row.is_empty()).collect();
        rows[rows.len().saturating_sub(3)..].join(" | ")
    }).unwrap_or_else(|error| format!("unreadable: {error}"))
}

fn login_vt(number: usize, password: &str) -> Verdict<String> {
    login_vt_as(number, LOGIN_NAME, password)
}

fn login_vt_as(number: usize, name: &str, password: &str) -> Verdict<String> {
    let began = libbreenix::suite::case_ms_left();
    wait_vt(number, "Breenix login: ")?;
    activate(number)?;
    type_line(name)?;
    wait_vt(number, "Password: ").map_err(|e| format!("{e}; screen ends: {}", screen_ends(number)))?;
    let password_at = libbreenix::suite::case_ms_left();
    type_line(password)?;
    let text = wait_vt(number, &format!("VTLOGIN tty{number} ")).map_err(|e| {
        format!("{e}; login began with {began}ms, password with {password_at}ms left; screen ends: {}", screen_ends(number))
    })?;
    require(!text.contains(password), "password appeared on the screen")?;
    Ok(text.lines().find(|line| line.starts_with("VTLOGIN ")).ok_or("missing login proof")?.trim().into())
}

fn getty_prompt_each_vt() -> CaseResult {
    measured(|| {
        let _init = start_gettys()?;
        for number in 1..=6 {
            wait_vt(number, "Breenix login: ")?;
            activate(number)?;
            let check = screen_check()?;
            identical(&check)?;
            require(check.lines().any(|l| l == format!("console foreground: tty{number}")), "wrong foreground")?;
            require(check.lines().any(|l| l == "console row: Breenix login:"), "displayed prompt missing")?;
        }
        activate(1)
    })
}

fn getty_wrong_password_refused() -> CaseResult {
    measured(|| {
        let account = LoginAccount::create()?;
        let _init = start_gettys()?;
        wait_vt(2, "Breenix login: ")?;
        activate(2)?;
        type_line(LOGIN_NAME)?;
        wait_vt(2, "Password: ")?;
        type_line("wrong")?;
        let text = wait_vt(2, "Login incorrect")?;
        require(!text.contains("VTLOGIN "), "wrong password started a session")?;
        account.restore()?;
        activate(1)
    })
}

fn getty_login_on_vt() -> CaseResult {
    measured(|| {
        let account = LoginAccount::create()?;
        let _init = start_gettys()?;
        login_vt(3, &account.password)?;
        account.restore()?;
        activate(1)
    })
}

fn getty_root_login_on_vt() -> CaseResult {
    measured(|| {
        let defs = std::fs::read_to_string("/etc/login.defs").map_err(|e| format!("read login.defs: {e}"))?;
        require(defs.lines().any(|l| l.split_whitespace().eq(["ROOT_LOGIN", "no"])), "login.defs does not ship ROOT_LOGIN no")?;
        let account = LoginAccount::root()?;
        let _init = start_gettys()?;
        let proof = login_vt_as(6, "root", &account.password)?;
        require(proof.split_whitespace().any(|w| w == "user=root") && proof.split_whitespace().any(|w| w == "uid=0"),
            &format!("root's session differs: {proof}"))?;
        account.restore()?;
        activate(1)
    })
}

fn getty_logout_reacquires() -> CaseResult {
    measured(|| {
        let account = LoginAccount::create()?;
        let _init = start_gettys()?;
        let before = login_vt(4, &account.password)?;
        // Remove the old prompt so it cannot be mistaken for a respawn.
        let fd = VtFd::open(4)?;
        io::write(fd.0, b"\x1bc").map_err(|e| e.to_string())?;
        let sid = before.split_whitespace().find(|word| word.starts_with("sid=")).ok_or("missing session")?;
        libbreenix::suite::hostkeys("ctrl-d")?;
        wait_vt(4, &format!("VTLOGOUT tty4 {sid}"))?;
        wait_vt(4, "Breenix login: ")?;
        let proof = login_vt(4, &account.password)?;
        require(proof != before, "logout did not create a new tty4 session")?;
        account.restore()?;
        activate(1)
    })
}

fn getty_independent_sessions() -> CaseResult {
    measured(|| {
        let account = LoginAccount::create()?;
        let _init = start_gettys()?;
        let first = login_vt(2, &account.password)?;
        let second = login_vt(5, &account.password)?;
        let session = |line: &str| line.split_whitespace().find(|part| part.starts_with("sid=")).unwrap_or("").to_string();
        require(session(&first) != session(&second), "two VTs shared a session")?;
        for proof in [&first, &second] {
            let sid: i32 = session(proof).strip_prefix("sid=").ok_or("missing session")?
                .parse().map_err(|e| format!("invalid session: {e}"))?;
            let actual = libbreenix::process::getsid(sid).map_err(|e| format!("login session exited: {e}"))?;
            require(actual.raw() as i32 == sid, "login process changed session")?;
        }
        // TIOCGSID is queried by the login shell on its own controlling
        // terminal; an outside session correctly receives ENOTTY for that query.
        require(vt_text(2)?.contains(&first) && vt_text(5)?.contains(&second),
            "a VT lost its independent login session")?;
        account.restore()?;
        activate(1)
    })
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let selected = args.windows(2).find_map(|pair| (pair[0] == "--case").then_some(pair[1].as_str()));
    if selected.is_none_or(|case| case == "getty" || case.starts_with("getty/")) {
        LOGIN_CREDENTIALS.get_or_init(login_credentials);
    }
    SUITE.run()
}
