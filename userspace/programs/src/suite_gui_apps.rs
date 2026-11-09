//! GUI application suite.
#[path = "guiapps_suite/pending.rs"]
mod pending;
#[path = "guiapps_suite/bterm.rs"]
mod bterm;
use breenix_suite::suite::{case, category, suite, Suite};

static SUITE: Suite = suite("gui-apps", "GUI applications", &[
    category("terminal-emulation", "bterm terminal emulation", &[
        case("vt-screen", "Scripted terminal screen", pending::not_built),
        case("scroll-region", "Scroll region", pending::not_built),
        case("alt-screen", "Alternate screen", pending::not_built),
        case("utf8", "UTF-8 text", pending::not_built),
        case("keys", "Terminal keys", pending::not_built),
    ]),
    category("bterm", "bterm", &[
        case("prompt", "Shell prompt", pending::not_built),
        case("command-output", "Command output", bterm::command_output),
        case("exit-status", "Exit status", pending::not_built),
        case("ctrl-c", "Interrupt foreground command", pending::not_built),
        case("tabs", "Tabs", pending::not_built),
        case("resize", "Resize", pending::not_built),
        case("scrollback", "Scrollback", pending::not_built),
        case("exec-tab", "Execute a command in a tab", pending::not_built),
        case("paste", "Paste", pending::not_built),
    ]),
]).gui().defer_display();

fn main() {
    SUITE.with_evidence_upload().run()
}
