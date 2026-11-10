//! GUI application suite.
#[path = "guiapps_suite/terminal_emulation.rs"]
mod terminal_emulation;
#[path = "guiapps_suite/bterm.rs"]
mod bterm;
#[path = "guiapps_suite/blauncher.rs"]
mod blauncher;
#[path = "guiapps_suite/blog.rs"]
mod blog;
#[path = "guiapps_suite/bfontpicker.rs"]
mod bfontpicker;
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
    category("blauncher", "blauncher", &[
        case("open", "Open launcher", blauncher::open).session(libuitest::supervisor::serve),
        case("filter", "Filter entries", blauncher::filter).session(libuitest::supervisor::serve),
        case("no-results", "No matches", blauncher::no_results).session(libuitest::supervisor::serve),
        case("keys-select", "Keyboard selection", blauncher::keys_select).session(libuitest::supervisor::serve),
        case("launch-gui", "Launch GUI app", blauncher::launch_gui).session(libuitest::supervisor::serve),
        case("launch-terminal", "Launch terminal app", blauncher::launch_terminal).session(libuitest::supervisor::serve),
        case("launch-missing", "Missing program", blauncher::launch_missing).session(libuitest::supervisor::serve),
        case("close", "Close launcher", blauncher::close).session(libuitest::supervisor::serve),
    ]),
    category("blog", "blog", &[
        case("tail", "Tail appended lines", blog::tail).session(libuitest::supervisor::serve),
        case("follow", "Follow new lines", blog::follow).session(libuitest::supervisor::serve),
        case("truncate-rotate", "Truncate and rotate", blog::truncate_rotate).session(libuitest::supervisor::serve),
        case("navigation", "Navigate log lines", blog::navigation).session(libuitest::supervisor::serve),
        case("search", "Search log lines", blog::search).session(libuitest::supervisor::serve),
        case("missing-file", "Wait for missing file", blog::missing_file).session(libuitest::supervisor::serve),
        case("blogd-records-once", "Record kernel messages once", blog::blogd_records_once).session(libuitest::supervisor::serve),
        case("blogd-restart", "Resume kernel messages after restart", blog::blogd_restart).session(libuitest::supervisor::serve),
    ]),
    category("bfontpicker", "bfontpicker", &[
        case("lists-fonts", "List fonts by metadata", bfontpicker::lists_fonts).session(libuitest::supervisor::serve),
        case("select-apply", "Select and apply a font", bfontpicker::select_apply).session(libuitest::supervisor::serve),
        case("persists", "Font selection persists", bfontpicker::persists).session(libuitest::supervisor::serve),
        case("keeps-unknown", "Comments, blank lines and key order survive a save", bfontpicker::keeps_unknown).session(libuitest::supervisor::serve),
    ]),
]).gui().defer_display();

fn main() {
    SUITE.with_evidence_upload().run()
}
