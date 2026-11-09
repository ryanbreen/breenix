//! UI testing suite skeleton; each unbuilt case fails with its planned layer.
#[path = "uitest_suite/capture_app.rs"]
mod capture_app;
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
#[path = "uitest_suite/tree_cases.rs"]
mod tree_cases;
#[path = "uitest_suite/canonical_cases.rs"]
mod canonical_cases;
#[path = "uitest_suite/capture_cases.rs"]
mod capture_cases;
#[path = "uitest_suite/tree.rs"]
mod tree;
#[path = "uitest_suite/wm_tree.rs"]
mod wm_tree;
#[path = "uitest_suite/input.rs"]
mod input;
#[path = "uitest_suite/input_time.rs"]
mod input_time;
#[path = "uitest_suite/input_delivery.rs"]
mod input_delivery;
#[path = "uitest_suite/clock.rs"]
mod clock;
#[path = "uitest_suite/timers.rs"]
mod timers;
#[path = "uitest_suite/settle.rs"]
mod settle;
#[path = "uitest_suite/settle_composite.rs"]
mod settle_composite;
#[path = "uitest_suite/proof.rs"]
mod proof;
use breenix_suite::suite::{case, category, fail, suite, CaseResult, Suite};

fn pending(layer: &str, step: u8) -> CaseResult {
    fail(format!("not built: {} (E22 step 22.{})", layer, step))
}

static SUITE: Suite = suite("ui-testing", "UI test framework", &[
    category("runner", "Proof executor & subreaper", &[
        // A clean scenario run in the inner executor returns Pass. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("inner-pass", "A clean scenario run in the inner executor returns Pass", inner::inner_pass),
        // A scenario with a planted pixel fault returns pixel-mismatch with its check and first pixel. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("inner-expected-failure", "A scenario with a planted pixel fault returns pixel-mismatch with its check and first pixel", capture_cases::inner_expected_failure),
        // In a boot session, an inner scenario and the real-runner fixture alike, a double-forked descendant that exits with status 7 after its parent has died is reparented to that session's supervisor (its parent pid read from `/proc/<pid>/stat` is the supervisor's) and reaped there, and the supervisor reports its pid and status 7 to the case; when the supervisor is killed first, the orphan goes to the runner, the runner writes `artifacts=none:supervisor-died`, and the case fails naming the dead supervisor. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("subreaper", "A double-forked orphan is reparented to and reaped by its session supervisor, or by the runner when the supervisor died first", sessions::subreaper).session(libuitest::supervisor::serve),
    ]),
    category("input", "Scripted keyboard, pointer & text input", &[
        // type_text("hello world") puts exactly that text in the focused text field. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("type-ascii", "type_text(\"hello world\") puts exactly that text in the focused text field", input::type_ascii).session(libuitest::supervisor::serve),
        // Characters that need Shift arrive correctly. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("type-shifted", "Characters that need Shift arrive correctly", input::type_shifted).session(libuitest::supervisor::serve),
        // Characters outside the keymap arrive through a text event. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("type-utf8", "Characters outside the keymap arrive through a text event", input::type_utf8).session(libuitest::supervisor::serve),
        // Newline and Tab arrive as the Enter and Tab keys. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("type-control", "Newline and Tab arrive as the Enter and Tab keys", input::type_control).session(libuitest::supervisor::serve),
        // chord(Ctrl, A) arrives as A with the Ctrl modifier. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("chord", "chord(Ctrl, A) arrives as A with the Ctrl modifier", input::chord).session(libuitest::supervisor::serve),
        // Holding a key for 600 ms of UI time gives exactly 3 repeats. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("hold-repeat", "Holding a key for 600 ms of UI time gives exactly 3 repeats", input_time::hold_repeat).session(libuitest::supervisor::serve),
        // click(node) lands on a visible point of the node, and the delivery names that node's window. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("click-node", "click(node) lands on a visible point of the node, and the delivery names that node's window", input::click_node).session(libuitest::supervisor::serve),
        // Clicking a node partly covered by another window lands on its exposed part, confirmed by hit test. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("click-partly-covered", "Clicking a node partly covered by another window lands on its exposed part, confirmed by hit test", input::click_partly_covered).session(libuitest::supervisor::serve),
        // Clicking a node clipped by a scrolled container uses its visible part. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("click-clipped", "Clicking a node clipped by a scrolled container uses its visible part", input::click_clipped).session(libuitest::supervisor::serve),
        // double_click delivers click counts 1 and 2. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("double-click", "double_click delivers click counts 1 and 2", input_time::double_click).session(libuitest::supervisor::serve),
        // A ten-step drag delivers ten motions in order between a press and a release; when the layout changes on the press and again mid-drag, and when the window moves mid-drag, every later motion and the release still reach the captured node through bwm's grab, the drag ends in exactly one completed gesture, and no button, pressed flag or capture is held at the end. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("drag-steps", "A ten-step drag reaches its captured node in order through layout changes and a window move, ending with nothing held", input::drag_steps).session(libuitest::supervisor::serve),
        // scroll(node, dy) delivers the wheel amount to the window under the node. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("scroll-node", "scroll(node, dy) delivers the wheel amount to the window under the node", input::scroll_node).session(libuitest::supervisor::serve),
        // Typing into a node focuses it first, and a node that cannot take focus fails by name. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("focus-then-type", "Typing into a node focuses it first, and a node that cannot take focus fails by name", input::focus_then_type).session(libuitest::supervisor::serve),
        // delivered(serial) returns only after the target window processed that serial. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("delivered", "delivered(serial) returns only after the target window processed that serial", input_delivery::delivered).session(libuitest::supervisor::serve),
        // An event sent to a hung app is reported unprocessed when the 2 s settle bound passes, naming the serial and the window. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("undelivered-fails", "An event sent to a hung app is reported unprocessed at the 2 s settle bound, naming the serial and window", settle::undelivered_fails).session(libuitest::supervisor::serve),
        // Every serial of a script ends with a terminal disposition. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("dispositions-all", "Every serial of a script ends with a terminal disposition", input_delivery::dispositions_all).session(libuitest::supervisor::serve),
        // A non-root session cannot inject, after a root session has. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("nonroot-denied", "A non-root session cannot inject, after a root session has", input_delivery::nonroot_denied).session(libuitest::supervisor::serve),
        // Ending a session releases every held key and button. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("teardown-releases", "Ending a session releases every held key and button", sessions::teardown_releases).session(libuitest::supervisor::serve),
        // Input arrives in order while the app animates. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("input-during-animation", "Input arrives in order while the app animates", settle_composite::input_during_animation).session(libuitest::supervisor::serve),
        // Input addressed to two windows of one app arrives at each. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("two-windows", "Input addressed to two windows of one app arrives at each", input_delivery::two_windows).session(libuitest::supervisor::serve),
        // A scripted Super double tap opens the launcher through bwm's real hotkey path. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("wm-hotkey-scripted", "A scripted Super double tap opens the launcher through bwm's real hotkey path", input_delivery::wm_hotkey_scripted).session(libuitest::supervisor::serve),
        // Events from a device outside the session's selection, modifier keys included, change nothing in the session. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("real-input-excluded", "Events from a device outside the session's selection, modifier keys included, change nothing in the session", input_delivery::real_input_excluded).session(libuitest::supervisor::serve),
    ]),
    category("tree", "Semantic trees of windows & widgets", &[
        // The test app's tree lists every widget with its expected role and key. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("roles-keys", "The test app's tree lists every widget with its expected role and key", tree::roles_keys).session(libuitest::supervisor::serve),
        // Names and values match the widgets' states set by the script. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("names-values", "Names and values match the widgets' states set by the script", tree::names_values).session(libuitest::supervisor::serve),
        // The focused flag moves with Tab. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("focus-flag", "The focused flag moves with Tab", tree::focus_flag).session(libuitest::supervisor::serve),
        // The focused widget draws the theme's focus ring at its bounds. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("focus-ring-pixels", "The focused widget draws the theme's focus ring at its bounds", tree::focus_ring_pixels).session(libuitest::supervisor::serve),
        // A disabled button is flagged not enabled. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("enabled-flag", "A disabled button is flagged not enabled", tree::enabled_flag).session(libuitest::supervisor::serve),
        // Checked, selected and pressed flags follow interaction. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("state-flags", "Checked, selected and pressed flags follow interaction", tree::state_flags).session(libuitest::supervisor::serve),
        // A text field's value equals the typed text. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("textfield-value", "A text field's value equals the typed text", tree::textfield_value).session(libuitest::supervisor::serve),
        // A terminal node's rows, cell attributes and cursor equal what its grid holds. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("terminal-grid", "A terminal node's rows, cell attributes and cursor equal what its grid holds", tree::terminal_grid).session(libuitest::supervisor::serve),
        // A tree and its pixels always come from the same commit. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("commit-coupled", "A tree and its pixels always come from the same commit", frames::commit_coupled).session(libuitest::supervisor::serve),
        // bwm's tree lists each window's title, bounds, z order, focus and minimized state. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("wm-tree", "bwm's tree lists each window's title, bounds, z order, focus and minimized state", wm_tree::wm_tree).session(libuitest::supervisor::serve),
        // Nodes scrolled out or covered are flagged not visible, with their clip. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("hidden-nodes", "Nodes scrolled out or covered are flagged not visible, with their clip", tree::hidden_nodes).session(libuitest::supervisor::serve),
        // Scrolling a list changes which items are flagged visible. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("list-scroll", "Scrolling a list changes which items are flagged visible", tree::list_scroll).session(libuitest::supervisor::serve),
        // An unrelated non-root process cannot map another app's tree. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("permission", "An unrelated non-root process cannot map another app's tree", tree_cases::permission).session(libuitest::supervisor::serve),
        // With no reader attached the tree is never serialized, and with one it is serialized once per commit. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("no-reader-no-serialize", "With no reader attached the tree is never serialized, and with one it is serialized once per commit", tree_cases::no_reader_no_serialize).session(libuitest::supervisor::serve),
        // A label whose tree text is "PASS" is checked against the certified renderer's "PASS" at its bounds. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("text-pixels", "A label whose tree text is PASS is checked against the certified renderer's PASS at its bounds", tree::text_pixels).session(libuitest::supervisor::serve),
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
        case("clock-frozen", "An app reading the UI clock sees it stand still between steps", clock::frozen).session(libuitest::supervisor::serve),
        // step(16) advances the app's clock by exactly 16 ms and the epoch by one. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("clock-step", "step(16) advances the app's clock by exactly 16 ms and the epoch by one", clock::step).session(libuitest::supervisor::serve),
        // Each step is acknowledged by every required participant in its commit metadata. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("epochs-acked", "Each step is acknowledged by every required participant in its commit metadata", clock::epochs_acked).session(libuitest::supervisor::serve),
        // An app's one-second timer fires on step(1000), not on real time. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("timers-virtual", "An app's one-second timer fires on step(1000), not on real time", timers::timers_virtual).session(libuitest::supervisor::serve),
        // Wall-clock time shown by apps and bwm is the fixed instant plus elapsed UI time. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("wall-time-fixed", "Wall-clock time shown by apps and bwm is the fixed instant plus elapsed UI time", clock::wall_time_fixed).session(libuitest::supervisor::serve),
        // bwm's taskbar clock and repeat timer read the UI clock. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("wm-clock", "bwm's taskbar clock and repeat timer read the UI clock", clock::wm_clock).session(libuitest::supervisor::serve),
        // Two apps in one session step together. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("shared-clock", "Two apps in one session step together", clock::shared).session(libuitest::supervisor::serve),
        // A stopped participant makes step fail naming it, unless the case excluded it. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("stopped-unacked-fails", "A stopped participant makes step fail naming it, unless the case excluded it", clock::stopped).session(libuitest::supervisor::serve),
        // Stepping never deadlocks against buffer back-pressure. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("backpressure-stepped", "Stepping never deadlocks against buffer back-pressure", timers::backpressure_stepped).session(libuitest::supervisor::serve),
        // An app that animates forever still lets every step complete. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("continuous-animation", "An app that animates forever still lets every step complete", timers::continuous_animation).session(libuitest::supervisor::serve),
        // An app's real-clock read counter stays 0 in a session and rises when the test app reads the real clock on purpose. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("real-clock-counter", "An app's real-clock read counter stays 0 in a session and rises when the test app reads the real clock on purpose", timers::real_clock_counter).session(libuitest::supervisor::serve),
        // Injected events carry UI time, and bwm's double-click threshold uses it. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("input-time", "Injected events carry UI time, and bwm's double-click threshold uses it", input_time::input_time).session(libuitest::supervisor::serve),
    ]),
    category("settle", "Causal barriers & settling", &[
        // settle returns once nothing changes. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("idle", "settle returns once nothing changes", settle::idle).session(libuitest::supervisor::serve),
        // settle waits until the target window commits with the serial processed, not until it reads the event. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("processed-not-consumed", "settle waits until the target window commits with the serial processed, not until it reads the event", settle::processed_not_consumed).session(libuitest::supervisor::serve),
        // An app that drains events and never commits makes settle fail, naming the window and the serial. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("drained-not-rendered", "An app that drains events and never commits makes settle fail, naming the window and the serial", settle::drained_not_rendered).session(libuitest::supervisor::serve),
        // An event that changes nothing settles through commit_no_change. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("no-change-ack", "An event that changes nothing settles through commit_no_change", settle::no_change_ack).session(libuitest::supervisor::serve),
        // settle waits until a completed composite lists every visible window at its newest pixel generation. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("composited", "settle waits until a completed composite lists every visible window at its newest pixel generation", settle_composite::composited).session(libuitest::supervisor::serve),
        // Settling after a commit_no_change completes through the composite that already holds the window's pixels, with no new composite. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("no-change-no-composite", "Settling after a commit_no_change completes through the composite that already holds the window's pixels", settle_composite::no_change_no_composite).session(libuitest::supervisor::serve),
        // An idle step settles through bwm's `none-needed` acknowledgment, with the composite counter unchanged. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("bwm-ack-no-composite", "An idle step settles through bwm's none-needed acknowledgment, with the composite counter unchanged", settle_composite::bwm_ack_no_composite).session(libuitest::supervisor::serve),
        // A hotkey settles with no client involved, through its WM-handled disposition. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("wm-handled", "A hotkey settles with no client involved, through its WM-handled disposition", settle_composite::wm_handled).session(libuitest::supervisor::serve),
        // settle waits for a named tree predicate on an asynchronous result. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("async-predicate", "settle waits for a named tree predicate on an asynchronous result", settle::async_predicate).session(libuitest::supervisor::serve),
        // A settle that cannot complete fails, naming the window, serial or predicate it waited for. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("timeout-names", "A settle that cannot complete fails, naming the window, serial or predicate it waited for", settle::timeout_names).session(libuitest::supervisor::serve),
        // settle returns while an app has a future animation deadline. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("animation-allowed", "settle returns while an app has a future animation deadline", settle_composite::animation_allowed).session(libuitest::supervisor::serve),
        // A minimized window still acknowledges epochs and processes its serials, settle waits for those, and settle does not wait for its pixels; once restored, the next settle waits for its current generation. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("minimized-acks", "A minimized window acknowledges and processes without being waited on for pixels until it is restored", settle_composite::minimized_acks).session(libuitest::supervisor::serve),
        // Repeated settle calls with nothing changing cause no extra frames. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("repeated-settle", "Repeated settle calls with nothing changing cause no extra frames", settle_composite::repeated_settle).session(libuitest::supervisor::serve),
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
        case("artifacts-on-mismatch", "A mismatch saves the got image, diff image, tree, input log and screen", diagnosis::artifacts_on_mismatch),
        // A passing case saves nothing unless saving is turned on. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("nothing-on-pass", "A passing case saves nothing unless saving is turned on", diagnosis::nothing_on_pass),
        // With saving turned on, a passing case saves its frames. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("save-all", "With saving turned on, a passing case saves its frames", diagnosis::save_all),
        // The saved input log lists every serial with its disposition. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("input-log", "The saved input log lists every serial with its disposition", diagnosis::input_log),
        // The saved screen is the readback at the failing step, with its record. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("screen-saved", "The saved screen is the readback at the failing step, with its record", diagnosis::screen_saved),
        // When the case process is killed, the runner sends `end collect`, waits for the supervisor's `done` before sweeping, and the evidence is on disk. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("case-death", "When the case process is killed, the runner sends end collect, waits for done before sweeping, and the evidence is on disk", diagnosis::case_death),
        // An app that dies right after a commit still has that commit and tree saved, from its remnant. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("app-death", "An app that dies right after a commit still has that commit and tree saved, from its remnant", diagnosis::app_death),
    ]),
    category("proof", "End-to-end scenarios & planted faults", &[
        // A widget drawn in the wrong color fails as pixel-mismatch with both colors. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("fault-wrong-color", "A widget drawn in the wrong color fails as pixel-mismatch with both colors", proof::fault_wrong_color),
        // An app that updates its tree but not its pixels fails as pixel-mismatch. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("fault-stale-pixels", "An app that updates its tree but not its pixels fails as pixel-mismatch", proof::fault_stale_pixels),
        // An app that drops one key fails as tree-mismatch on the text field's value. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("fault-drop-key", "An app that drops one key fails as tree-mismatch on the text field's value", proof::fault_drop_key),
        // A hung app fails as app-hung when the 2 s step bound passes, instead of hanging the suite. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("fault-hang", "A hung app fails as app-hung when the 2 s step bound passes, instead of hanging the suite", proof::fault_hang),
        // Filling the test app's form (text field, checkbox, slider, submit) gives the expected tree and frame. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("form-scenario", "Filling the test app's form (text field, checkbox, slider, submit) gives the expected tree and frame", proof::form_scenario).checks(&["initial", "typed", "checked", "slider", "submitted"]),
        // Opening a menu and choosing an item gives the expected tree and frame. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("menu-scenario", "Opening a menu and choosing an item gives the expected tree and frame", proof::menu_scenario).checks(&["initial", "opened", "chosen"]),
        // Scrolling and selecting in a list gives the expected tree and frame. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("list-scenario", "Scrolling and selecting in a list gives the expected tree and frame", proof::list_scenario).checks(&["initial", "scrolled", "selected"]),
        // A scripted focus scenario on bwm gives the expected trees and screens. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("wm-focus-scenario", "A scripted focus scenario on bwm gives the expected trees and screens", proof::wm_focus_scenario),
        // A scripted stacking scenario on bwm gives the expected trees and screens. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("wm-stack-scenario", "A scripted stacking scenario on bwm gives the expected trees and screens", || pending("proof", 7)),
    ]),
]).gui().defer_display();

fn main() {
    // Prime the cache without bypassing the runner. Each session independently
    // verifies these pins and reports any failure through its case verdict.
    let _ = libuitest::fixtures::verify_fonts(std::path::Path::new("/usr/share/fonts"));
    breenix_suite::suite::set_painter(Box::new(stage_painter::StagePainter::new()));
    SUITE.with_evidence_upload().run()
}
