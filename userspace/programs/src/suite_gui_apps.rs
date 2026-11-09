//! GUI application suite.
#[path = "guiapps_suite/pending.rs"]
mod pending;
#[path = "guiapps_suite/terminal_emulation.rs"]
mod terminal_emulation;
#[path = "guiapps_suite/bterm.rs"]
mod bterm;
use breenix_suite::suite::{case, category, suite, Suite};

static SUITE: Suite = suite("gui-apps", "GUI applications", &[
    category("terminal-emulation", "bterm terminal emulation", &[
        case("vt-screen", "Scripted terminal screen", terminal_emulation::vt_screen),
        case("scroll-region", "Scroll region", terminal_emulation::scroll_region),
        case("alt-screen", "Alternate screen", terminal_emulation::alt_screen),
        case("utf8", "UTF-8 text", terminal_emulation::utf8),
        case("keys", "Terminal keys", terminal_emulation::keys),
    ]),
    category("bterm", "bterm", &[
        case("prompt", "Shell prompt", bterm::prompt),
        case("command-output", "Command output", bterm::command_output),
        case("exit-status", "Exit status", bterm::exit_status),
        case("ctrl-c", "Interrupt foreground command", bterm::ctrl_c),
        case("tabs", "Tabs", bterm::tabs),
        case("resize", "Resize", bterm::resize),
        case("scrollback", "Scrollback", pending::not_built),
        case("exec-tab", "Execute a command in a tab", bterm::exec_tab),
        case("paste", "Paste", pending::not_built),
    ]),
]).gui().defer_display();

fn main() {
    SUITE.with_evidence_upload().run()
}
