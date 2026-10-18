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

const SCREEN: Layer = Layer { name: "the console core on the screen (V1)", probe: Probe::ConsoleCheck };
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

/// One function per case that has no check yet, each failing with the layer it needs.
macro_rules! needs {
    ($($name:ident => $layer:ident;)*) => {
        $(fn $name() -> CaseResult { missing(&$layer) })*
    };
}

needs! {
    screen_tty1_on_screen => SCREEN;
    screen_readback_identical => SCREEN;
    screen_scanout_bound => SCREEN;
    screen_font_oracle => SCREEN;
    screen_size_class => SCREEN;
    screen_cursor_drawn => SCREEN;
    screen_log_on_tty1 => SCREEN;
    screen_log_quiet_in_session => SCREEN;
    screen_display_taken_stops_drawing => SCREEN;
    screen_display_released_redraws => SCREEN;
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
