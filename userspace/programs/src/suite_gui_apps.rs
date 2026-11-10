//! GUI application suite.
#[path = "guiapps_suite/terminal_emulation.rs"]
mod terminal_emulation;
#[path = "guiapps_suite/bterm.rs"]
mod bterm;
use breenix_suite::suite::{case, category, suite, Suite};

static SUITE: Suite = suite("gui-apps", "GUI applications", &[
    category("terminal-emulation", "bterm terminal emulation", &[
        case("vt-screen", "Scripted terminal screen", terminal_emulation::vt_screen).session(libuitest::supervisor::serve),
        case("scroll-region", "Scroll region", terminal_emulation::scroll_region).session(libuitest::supervisor::serve),
        case("alt-screen", "Alternate screen", terminal_emulation::alt_screen).session(libuitest::supervisor::serve),
        case("utf8", "UTF-8 text", terminal_emulation::utf8).session(libuitest::supervisor::serve),
        case("keys", "Terminal keys", terminal_emulation::keys).session(libuitest::supervisor::serve),
    ]),
    category("bterm", "bterm", &[
        case("prompt", "Shell prompt", bterm::prompt).session(libuitest::supervisor::serve),
        case("command-output", "Command output", bterm::command_output).session(libuitest::supervisor::serve),
        case("exit-status", "Exit status", bterm::exit_status).session(libuitest::supervisor::serve),
        case("ctrl-c", "Interrupt foreground command", bterm::ctrl_c).session(libuitest::supervisor::serve),
        case("tabs", "Tabs", bterm::tabs).session(libuitest::supervisor::serve),
        case("resize", "Resize", bterm::resize).session(libuitest::supervisor::serve),
        case("scrollback", "Scrollback", bterm::scrollback).session(libuitest::supervisor::serve),
        case("exec-tab", "Execute a command in a tab", bterm::exec_tab).session(libuitest::supervisor::serve),
        case("paste", "Paste clipboard text", bterm::paste).session(libuitest::supervisor::serve),
    ]),
]).gui().defer_display();

fn main() {
    SUITE.with_evidence_upload().run()
}
