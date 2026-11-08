//! UI testing suite skeleton; each unbuilt case fails with its planned layer.
#[path = "uitest_suite/runner.rs"]
mod runner;
#[path = "uitest_suite/inner.rs"]
mod inner;
#[path = "uitest_suite/sessions.rs"]
mod sessions;
#[path = "uitest_suite/diagnosis.rs"]
mod diagnosis;
#[path = "uitest_suite/config.rs"]
mod config;
#[path = "suite_breengel/stage_painter.rs"]
mod stage_painter;
#[path = "uitest_suite/comparison_cases.rs"]
mod comparison_cases;
#[path = "uitest_suite/frames.rs"]
mod frames;
#[path = "uitest_suite/canonical_cases.rs"]
mod canonical_cases;
#[path = "uitest_suite/capture_cases.rs"]
mod capture_cases;
use breenix_suite::suite::{case, category, fail, suite, CaseResult, Suite};

fn pending(layer: &str, step: u8) -> CaseResult {
    fail(format!("not built: {} (E22 step 22.{})", layer, step))
}

static SUITE: Suite = suite("ui-testing", "UI test framework", &[
    category("runner", "Result transport & proof executor", &[
        // A clean scenario run in the inner executor returns Pass. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("inner-pass", "A clean scenario run in the inner executor returns Pass", inner::inner_pass),
        // A scenario with a planted pixel fault returns pixel-mismatch with its check and first pixel. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("inner-expected-failure", "A scenario with a planted pixel fault returns pixel-mismatch with its check and first pixel", capture_cases::inner_expected_failure),
        // In a boot session, an inner scenario and the real-runner fixture alike, a double-forked descendant that exits with status 7 after its parent has died is reparented to that session's supervisor (its parent pid read from `/proc/<pid>/stat` is the supervisor's) and reaped there, and the supervisor reports its pid and status 7 to the case; when the supervisor is killed first, the orphan goes to the runner, the runner writes `artifacts=none:supervisor-died`, and the case fails naming the dead supervisor. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("subreaper", "A double-forked orphan is reparented to and reaped by its session supervisor, or by the runner when the supervisor died first", sessions::subreaper).session(libuitest::supervisor::serve),
    ]),
    category("input", "Scripted keyboard, pointer & text input", &[
        // type_text("hello world") puts exactly that text in the focused text field. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("type-ascii", "type_text(\"hello world\") puts exactly that text in the focused text field", || pending("input", 6)),
        // Characters that need Shift arrive correctly. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("type-shifted", "Characters that need Shift arrive correctly", || pending("input", 6)),
        // Characters outside the keymap arrive through a text event. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("type-utf8", "Characters outside the keymap arrive through a text event", || pending("input", 6)),
        // Newline and Tab arrive as the Enter and Tab keys. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("type-control", "Newline and Tab arrive as the Enter and Tab keys", || pending("input", 6)),
        // chord(Ctrl, A) arrives as A with the Ctrl modifier. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("chord", "chord(Ctrl, A) arrives as A with the Ctrl modifier", || pending("input", 6)),
        // Holding a key for 600 ms of UI time gives exactly 3 repeats. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("hold-repeat", "Holding a key for 600 ms of UI time gives exactly 3 repeats", || pending("input", 6)),
        // click(node) lands on a visible point of the node, and the delivery names that node's window. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("click-node", "click(node) lands on a visible point of the node, and the delivery names that node's window", || pending("input", 6)),
        // Clicking a node partly covered by another window lands on its exposed part, confirmed by hit test. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("click-partly-covered", "Clicking a node partly covered by another window lands on its exposed part, confirmed by hit test", || pending("input", 6)),
        // Clicking a node clipped by a scrolled container uses its visible part. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("click-clipped", "Clicking a node clipped by a scrolled container uses its visible part", || pending("input", 6)),
        // double_click delivers click counts 1 and 2. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("double-click", "double_click delivers click counts 1 and 2", || pending("input", 6)),
        // A ten-step drag delivers ten motions in order between a press and a release; when the layout changes on the press and again mid-drag, and when the window moves mid-drag, every later motion and the release still reach the captured node through bwm's grab, the drag ends in exactly one completed gesture, and no button, pressed flag or capture is held at the end. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("drag-steps", "A ten-step drag reaches its captured node in order through layout changes and a window move, ending with nothing held", || pending("input", 6)),
        // scroll(node, dy) delivers the wheel amount to the window under the node. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("scroll-node", "scroll(node, dy) delivers the wheel amount to the window under the node", || pending("input", 6)),
        // Typing into a node focuses it first, and a node that cannot take focus fails by name. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("focus-then-type", "Typing into a node focuses it first, and a node that cannot take focus fails by name", || pending("input", 6)),
        // delivered(serial) returns only after the target window processed that serial. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("delivered", "delivered(serial) returns only after the target window processed that serial", || pending("input", 6)),
        // An event sent to a hung app is reported unprocessed when the 2 s settle bound passes, naming the serial and the window. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("undelivered-fails", "An event sent to a hung app is reported unprocessed at the 2 s settle bound, naming the serial and window", || pending("input", 6)),
        // Every serial of a script ends with a terminal disposition. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("dispositions-all", "Every serial of a script ends with a terminal disposition", || pending("input", 6)),
        // A non-root session cannot inject, after a root session has. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("nonroot-denied", "A non-root session cannot inject, after a root session has", || pending("input", 6)),
        // Ending a session releases every held key and button. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("teardown-releases", "Ending a session releases every held key and button", sessions::teardown_releases).session(libuitest::supervisor::serve),
        // Input arrives in order while the app animates. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("input-during-animation", "Input arrives in order while the app animates", || pending("input", 6)),
        // Input addressed to two windows of one app arrives at each. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("two-windows", "Input addressed to two windows of one app arrives at each", || pending("input", 6)),
        // A scripted Super double tap opens the launcher through bwm's real hotkey path. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("wm-hotkey-scripted", "A scripted Super double tap opens the launcher through bwm's real hotkey path", || pending("input", 6)),
        // Events from a device outside the session's selection, modifier keys included, change nothing in the session. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("real-input-excluded", "Events from a device outside the session's selection, modifier keys included, change nothing in the session", || pending("input", 6)),
    ]),
    category("tree", "Semantic trees of windows & widgets", &[
        // The test app's tree lists every widget with its expected role and key. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("roles-keys", "The test app's tree lists every widget with its expected role and key", || pending("tree", 6)),
        // Names and values match the widgets' states set by the script. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("names-values", "Names and values match the widgets' states set by the script", || pending("tree", 6)),
        // The focused flag moves with Tab. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("focus-flag", "The focused flag moves with Tab", || pending("tree", 6)),
        // The focused widget draws the theme's focus ring at its bounds. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("focus-ring-pixels", "The focused widget draws the theme's focus ring at its bounds", || pending("tree", 6)),
        // A disabled button is flagged not enabled. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("enabled-flag", "A disabled button is flagged not enabled", || pending("tree", 6)),
        // Checked, selected and pressed flags follow interaction. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("state-flags", "Checked, selected and pressed flags follow interaction", || pending("tree", 6)),
        // A text field's value equals the typed text. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("textfield-value", "A text field's value equals the typed text", || pending("tree", 6)),
        // A terminal node's rows, cell attributes and cursor equal what its grid holds. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("terminal-grid", "A terminal node's rows, cell attributes and cursor equal what its grid holds", || pending("tree", 6)),
        // A tree and its pixels always come from the same commit. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("commit-coupled", "A tree and its pixels always come from the same commit", || pending("tree", 6)),
        // bwm's tree lists each window's title, bounds, z order, focus and minimized state. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("wm-tree", "bwm's tree lists each window's title, bounds, z order, focus and minimized state", || pending("tree", 6)),
        // Nodes scrolled out or covered are flagged not visible, with their clip. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("hidden-nodes", "Nodes scrolled out or covered are flagged not visible, with their clip", || pending("tree", 6)),
        // Scrolling a list changes which items are flagged visible. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("list-scroll", "Scrolling a list changes which items are flagged visible", || pending("tree", 6)),
        // An unrelated non-root process cannot map another app's tree. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("permission", "An unrelated non-root process cannot map another app's tree", || pending("tree", 6)),
        // With no reader attached the tree is never serialized, and with one it is serialized once per commit. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("no-reader-no-serialize", "With no reader attached the tree is never serialized, and with one it is serialized once per commit", || pending("tree", 6)),
        // A label whose tree text is "PASS" is checked against the certified renderer's "PASS" at its bounds. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("text-pixels", "A label whose tree text is PASS is checked against the certified renderer's PASS at its bounds", || pending("tree", 6)),
    ]),
    category("frames", "Frame capture & comparison", &[
        // A window capture equals the committed buffer of that commit. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("window-capture", "A window capture equals the committed buffer of that commit", capture_cases::window_capture).session(libuitest::supervisor::serve),
        // Capturing commit N while the app draws N+1 returns N whole. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("capture-while-drawing", "Capturing commit N while the app draws N+1 returns N whole", frames::capture_while_drawing).session(libuitest::supervisor::serve),
        // A resize during a capture gives the whole old commit, pixels and tree, never ENOENT or a mixture. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("capture-resize-race", "A resize during a capture gives the whole old commit, pixels and tree, never ENOENT or a mixture", frames::capture_resize_race).session(libuitest::supervisor::serve),
        // A screen capture shows the window at its position, with its observation record. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("screen-capture", "A screen capture shows the window at its position, with its observation record", capture_cases::screen_capture).session(libuitest::supervisor::serve),
        // A screen capture lists the commit of each window it contains. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("capture-commits", "A screen capture lists the commit of each window it contains", capture_cases::capture_commits).session(libuitest::supervisor::serve),
        // Captures taken with the cursor hidden contain no cursor pixels. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("cursor-excluded", "Captures taken with the cursor hidden contain no cursor pixels", canonical_cases::cursor_excluded).session(libuitest::supervisor::serve),
        // The test app's canonical scene hashes to the pinned value on this target. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("cross-target", "The test app's canonical scene hashes to the pinned value on this target", canonical_cases::cross_target).session(libuitest::supervisor::serve),
        // Comparing two 1280x800 frames is measured with a 500 ms sanity cap. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("compare-cost", "Comparing two 1280x800 frames is measured with a 500 ms sanity cap", || comparison_cases::compare_cost().map_err(Into::into)),
    ]),
    category("clock", "UI time & stepping", &[
        // An app reading the UI clock sees it stand still between steps. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("clock-frozen", "An app reading the UI clock sees it stand still between steps", || pending("clock", 7)),
        // step(16) advances the app's clock by exactly 16 ms and the epoch by one. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("clock-step", "step(16) advances the app's clock by exactly 16 ms and the epoch by one", || pending("clock", 7)),
        // Each step is acknowledged by every required participant in its commit metadata. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("epochs-acked", "Each step is acknowledged by every required participant in its commit metadata", || pending("clock", 7)),
        // An app's one-second timer fires on step(1000), not on real time. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("timers-virtual", "An app's one-second timer fires on step(1000), not on real time", || pending("clock", 7)),
        // Wall-clock time shown by apps and bwm is the fixed instant plus elapsed UI time. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("wall-time-fixed", "Wall-clock time shown by apps and bwm is the fixed instant plus elapsed UI time", || pending("clock", 7)),
        // bwm's taskbar clock and repeat timer read the UI clock. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("wm-clock", "bwm's taskbar clock and repeat timer read the UI clock", || pending("clock", 7)),
        // Two apps in one session step together. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("shared-clock", "Two apps in one session step together", || pending("clock", 7)),
        // A stopped participant makes step fail naming it, unless the case excluded it. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("stopped-unacked-fails", "A stopped participant makes step fail naming it, unless the case excluded it", || pending("clock", 7)),
        // Stepping never deadlocks against buffer back-pressure. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("backpressure-stepped", "Stepping never deadlocks against buffer back-pressure", || pending("clock", 7)),
        // An app that animates forever still lets every step complete. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("continuous-animation", "An app that animates forever still lets every step complete", || pending("clock", 7)),
        // An app's real-clock read counter stays 0 in a session and rises when the test app reads the real clock on purpose. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("real-clock-counter", "An app's real-clock read counter stays 0 in a session and rises when the test app reads the real clock on purpose", || pending("clock", 7)),
        // Injected events carry UI time, and bwm's double-click threshold uses it. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("input-time", "Injected events carry UI time, and bwm's double-click threshold uses it", || pending("clock", 7)),
    ]),
    category("settle", "Causal barriers & settling", &[
        // settle returns once nothing changes. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("idle", "settle returns once nothing changes", || pending("settle", 7)),
        // settle waits until the target window commits with the serial processed, not until it reads the event. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("processed-not-consumed", "settle waits until the target window commits with the serial processed, not until it reads the event", || pending("settle", 7)),
        // An app that drains events and never commits makes settle fail, naming the window and the serial. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("drained-not-rendered", "An app that drains events and never commits makes settle fail, naming the window and the serial", || pending("settle", 7)),
        // An event that changes nothing settles through commit_no_change. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("no-change-ack", "An event that changes nothing settles through commit_no_change", || pending("settle", 7)),
        // settle waits until a completed composite lists every visible window at its newest pixel generation. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("composited", "settle waits until a completed composite lists every visible window at its newest pixel generation", || pending("settle", 7)),
        // Settling after a commit_no_change completes through the composite that already holds the window's pixels, with no new composite. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("no-change-no-composite", "Settling after a commit_no_change completes through the composite that already holds the window's pixels", || pending("settle", 7)),
        // An idle step settles through bwm's `none-needed` acknowledgment, with the composite counter unchanged. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("bwm-ack-no-composite", "An idle step settles through bwm's none-needed acknowledgment, with the composite counter unchanged", || pending("settle", 7)),
        // A hotkey settles with no client involved, through its WM-handled disposition. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("wm-handled", "A hotkey settles with no client involved, through its WM-handled disposition", || pending("settle", 7)),
        // settle waits for a named tree predicate on an asynchronous result. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("async-predicate", "settle waits for a named tree predicate on an asynchronous result", || pending("settle", 7)),
        // A settle that cannot complete fails, naming the window, serial or predicate it waited for. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("timeout-names", "A settle that cannot complete fails, naming the window, serial or predicate it waited for", || pending("settle", 7)),
        // settle returns while an app has a future animation deadline. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("animation-allowed", "settle returns while an app has a future animation deadline", || pending("settle", 7)),
        // A minimized window still acknowledges epochs and processes its serials, settle waits for those, and settle does not wait for its pixels; once restored, the next settle waits for its current generation. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("minimized-acks", "A minimized window acknowledges and processes without being waited on for pixels until it is restored", || pending("settle", 7)),
        // Repeated settle calls with nothing changing cause no extra frames. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("repeated-settle", "Repeated settle calls with nothing changing cause no extra frames", || pending("settle", 7)),
    ]),
    category("sessions", "Sessions, state isolation & cleanup", &[
        // A desktop session with one app measures time to its first composited commit with a 2 s sanity cap. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("start-desktop", "A desktop session with one app measures time to its first composited commit with a 2 s sanity cap", sessions::start_desktop).session(libuitest::supervisor::serve).collects(),
        // A kiosk session with one app measures time to its first composited commit with a 2 s sanity cap. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("start-kiosk", "A kiosk session with one app measures time to its first composited commit with a 2 s sanity cap", sessions::start_kiosk).session(libuitest::supervisor::serve).collects(),
        // Apps in a session receive its fonts, clock, seed, mode, time zone, locale and config root. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("environment", "Apps in a session receive its fonts, clock, seed, mode, time zone, locale and config root", sessions::environment).session(libuitest::supervisor::serve),
        // bwm, Breengel's font configuration and every app read and write under the session's config root. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("config-root", "bwm, Breengel's font configuration and every app read and write under the session's config root", config::config_root).session(libuitest::supervisor::serve),
        // A session that changes every setting leaves the global configuration files unchanged. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("global-untouched", "A session that changes every setting leaves the global configuration files unchanged", config::global_untouched).session(libuitest::supervisor::serve),
        // A case killed while changing settings is followed by a case that sees the defaults. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("restore-after-crash", "A case killed while changing settings is followed by a case that sees the defaults", config::restore_after_crash),
        // Two settings scenarios give the same results in either order. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("order-independent", "Two settings scenarios give the same results in either order", config::order_independent),
        // After a case its virtual devices are gone and no key or button is held. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("devices-reset", "After a case its virtual devices are gone and no key or button is held", sessions::devices_reset).session(libuitest::supervisor::serve),
        // After a case, no session process remains and the runner owns the display. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("teardown", "After a case, no session process remains and the runner owns the display", sessions::teardown),
        // A hung app is killed and reaped at teardown. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("teardown-hung", "A hung app is killed and reaped at teardown", sessions::teardown_hung).session(libuitest::supervisor::serve),
        // A screen capture during a case contains no panel pixels. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("panel-not-in-case", "A screen capture during a case contains no panel pixels", sessions::panel_not_in_case).session(libuitest::supervisor::serve),
        // When a session's compositor dies, the display goes to the session supervisor, then to the runner at case end. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("ownership-stack", "When a session's compositor dies, the display goes to the session supervisor, then to the runner at case end", sessions::ownership_stack),
        // A scenario that runs out of time fails, and its evidence is saved inside the reserve. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("case-timeout", "A scenario that runs out of time fails, and its evidence is saved inside the reserve", sessions::case_timeout),
        // An app crashing during a scenario fails it, naming the app and the signal. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("app-crash", "An app crashing during a scenario fails it, naming the app and the signal", sessions::app_crash),
        // An app launched as uid 1000 runs in the session while the harness keeps root observation. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("unprivileged-app", "An app launched as uid 1000 runs in the session while the harness keeps root observation", sessions::unprivileged_app).session(libuitest::supervisor::serve),
        // Twenty sessions in a row leave windows, processes and memory at the baseline. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("twenty-sessions", "Twenty sessions in a row leave windows, processes and memory at the baseline", sessions::twenty_sessions),
        // wait_window(title) returns after that window's first composited commit. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("wait-window", "wait_window(title) returns after that window's first composited commit", sessions::wait_window).session(libuitest::supervisor::serve),
        // wait_window for a window that never appears fails, naming the title. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("wait-window-timeout", "wait_window for a window that never appears fails, naming the title", sessions::wait_window_timeout).session(libuitest::supervisor::serve),
        // launch passes argv and the environment exactly. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("launch-argv", "launch passes argv and the environment exactly", sessions::launch_argv).session(libuitest::supervisor::serve),
    ]),
    category("diagnosis", "Saved evidence & failure messages", &[
        // A mismatch saves the got image, diff image, tree, input log and screen. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("artifacts-on-mismatch", "A mismatch saves the got image, diff image, tree, input log and screen", || pending("diagnosis", 7)),
        // A passing case saves nothing unless saving is turned on. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("nothing-on-pass", "A passing case saves nothing unless saving is turned on", || pending("diagnosis", 7)),
        // With saving turned on, a passing case saves its frames. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("save-all", "With saving turned on, a passing case saves its frames", || pending("diagnosis", 7)),
        // The saved input log lists every serial with its disposition. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("input-log", "The saved input log lists every serial with its disposition", || pending("diagnosis", 7)),
        // The saved screen is the readback at the failing step, with its record. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("screen-saved", "The saved screen is the readback at the failing step, with its record", || pending("diagnosis", 7)),
        // When the case process is killed, the runner sends `end collect`, waits for the supervisor's `done` before sweeping, and the evidence is on disk. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("case-death", "When the case process is killed, the runner sends end collect, waits for done before sweeping, and the evidence is on disk", diagnosis::case_death),
        // An app that dies right after a commit still has that commit and tree saved, from its remnant. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("app-death", "An app that dies right after a commit still has that commit and tree saved, from its remnant", || pending("diagnosis", 7)),
    ]),
    category("proof", "Proven on bwm & a test app", &[
        // A widget drawn in the wrong color fails as pixel-mismatch with both colors. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("fault-wrong-color", "A widget drawn in the wrong color fails as pixel-mismatch with both colors", || pending("proof", 7)),
        // An app that updates its tree but not its pixels fails as pixel-mismatch. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("fault-stale-pixels", "An app that updates its tree but not its pixels fails as pixel-mismatch", || pending("proof", 7)),
        // An app that drops one key fails as tree-mismatch on the text field's value. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("fault-drop-key", "An app that drops one key fails as tree-mismatch on the text field's value", || pending("proof", 7)),
        // A hung app fails as app-hung when the 2 s step bound passes, instead of hanging the suite. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("fault-hang", "A hung app fails as app-hung when the 2 s step bound passes, instead of hanging the suite", || pending("proof", 7)),
        // Filling the test app's form (text field, checkbox, slider, submit) gives the expected tree and frame. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("form-scenario", "Filling the test app's form (text field, checkbox, slider, submit) gives the expected tree and frame", || pending("proof", 7)),
        // Opening a menu and choosing an item gives the expected tree and frame. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("menu-scenario", "Opening a menu and choosing an item gives the expected tree and frame", || pending("proof", 7)),
        // Scrolling and selecting in a list gives the expected tree and frame. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("list-scenario", "Scrolling and selecting in a list gives the expected tree and frame", || pending("proof", 7)),
        // A scripted focus scenario on bwm gives the expected trees and screens. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("wm-focus-scenario", "A scripted focus scenario on bwm gives the expected trees and screens", || pending("proof", 7)),
        // A scripted stacking scenario on bwm gives the expected trees and screens. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("wm-stack-scenario", "A scripted stacking scenario on bwm gives the expected trees and screens", || pending("proof", 7)),
    ]),
]).gui().defer_display();

fn main() {
    if let Err(error) = libuitest::fixtures::verify_fonts(std::path::Path::new("/usr/share/fonts")) {
        eprintln!("ui-testing fixture verification: {error}");
        std::process::exit(1);
    }
    breenix_suite::suite::set_painter(Box::new(stage_painter::StagePainter::new()));
    SUITE.with_evidence_upload().run()
}
