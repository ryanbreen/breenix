//! UI testing suite skeleton; each unbuilt case fails with its planned layer.
#[path = "uitest_suite/runner.rs"]
mod runner;
#[path = "uitest_suite/inner.rs"]
mod inner;
#[path = "uitest_suite/sessions.rs"]
mod sessions;
#[path = "uitest_suite/config.rs"]
mod config;
#[path = "suite_breengel/stage_painter.rs"]
mod stage_painter;
use libuitest::schema as frame_schema;
use breenix_suite::suite as frame_case;
#[path = "uitest_suite/frames.rs"]
mod frames;
use breenix_suite::suite::{case, category, fail, suite, CaseResult, Suite};

fn pending(layer: &str, step: u8) -> CaseResult {
    fail(format!("not built: {} (E22 step 22.{})", layer, step))
}

static SUITE: Suite = suite("ui-testing", "UI test framework", &[
    category("runner", "Result transport & proof executor", &[
        // Records reach the runner with the live strip turned off. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("transport-always", "Records reach the runner with the live strip turned off", runner::transport_always),
        // A writer that fills the pipe blocks, and all 256 records arrive. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("transport-blocking", "A writer that fills the pipe blocks, and all 256 records arrive", runner::transport_blocking),
        // A record written immediately before the case exits still arrives. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("transport-exit-race", "A record written immediately before the case exits still arrives", runner::transport_exit_race),
        // A suite table declaring more checks for one case than the record limit is refused at suite start. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("transport-capacity", "A suite table declaring more checks for one case than the record limit is refused at suite start", runner::transport_capacity),
        // Under the real runner in the scoped fixture (section 8.4), a record naming another case or an undeclared check makes that runner report the case FAIL. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("record-binding", "Under the real runner in the fixture, a record naming another case or an undeclared check fails the case", runner::record_binding),
        // Under the real runner in the fixture, a malformed record makes it report the case FAIL. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("record-malformed", "Under the real runner in the fixture, a malformed record fails the case", runner::record_malformed),
        // Under the real runner in the fixture, a second record for the same check makes it report the case FAIL. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("record-duplicate", "Under the real runner in the fixture, a second record for the same check fails the case", runner::record_duplicate),
        // An app the case execs cannot write records. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("forgery-exec", "An app the case execs cannot write records", runner::forgery_exec),
        // A helper created through `suite::spawn` that never execs cannot write records. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("forgery-fork", "A helper created through suite::spawn that never execs cannot write records", runner::forgery_fork),
        // Under the real runner in the fixture, a case that declares a frame check and never runs it is reported FAIL, naming the check. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("never-ran", "Under the real runner in the fixture, a declared frame check that never runs fails the case, naming the check", runner::never_ran),
        // Under the real runner in the fixture, a case that returns Ok after a mismatch record is reported FAIL. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("mismatch-is-fail", "Under the real runner in the fixture, a case that returns Ok after a mismatch record fails", runner::mismatch_is_fail),
        // The fixture's clean case passes under the real runner, and the fixture's transcript ends in a consistent DONE. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("outer-positive", "The fixture's clean case passes under the real runner and its transcript ends in a consistent DONE", runner::outer_positive),
        // A fixture case killed mid-scenario is reported FAIL by the real runner, its supervisor's COLLECT and `done` arrive before the sweep, and the evidence it names is on disk. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("outer-case-death", "A fixture case killed mid-scenario fails, its COLLECT and done arrive before the sweep, and its evidence is on disk", runner::outer_case_death),
        // A fixture supervisor that never answers is swept at the case limit, the runner writes `artifacts=none:collector-timeout`, and the case's primary failure stands; a fixture supervisor that dies instead makes the runner write `artifacts=none:supervisor-died` and reap the session's orphans. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("outer-collector-timeout", "A silent fixture supervisor is swept at the limit as collector-timeout, and a dead one is recorded supervisor-died with its orphans reaped", runner::outer_collector_timeout),
        // The real runner, run as a subreaper, kills and reaps every descendant of the fixture, a double-forked helper included, and nothing outside it. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("sweep-descendants", "The real runner as a subreaper kills and reaps every descendant of the fixture and nothing outside it", runner::sweep_descendants),
        // A clean scenario run in the inner executor returns Pass. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("inner-pass", "A clean scenario run in the inner executor returns Pass", inner::inner_pass),
        // A scenario with a planted pixel fault returns pixel-mismatch with its check and first pixel. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("inner-expected-failure", "A scenario with a planted pixel fault returns pixel-mismatch with its check and first pixel", || pending("runner", 4)),
        // An outer case expecting pixel-mismatch fails when the scenario fails another way. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("inner-wrong-class", "An outer case expecting pixel-mismatch fails when the scenario fails another way", inner::inner_wrong_class),
        // A scenario whose app crashes is not accepted as detecting a pixel fault. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("inner-crash-not-detection", "A scenario whose app crashes is not accepted as detecting a pixel fault", inner::inner_crash_not_detection),
        // Inner teardown kills only the scenario's descendants, and the outer case's helpers survive. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("inner-teardown-scoped", "Inner teardown kills only the scenario's descendants, and the outer case's helpers survive", inner::inner_teardown_scoped),
        // An inner scenario's records never appear in the outer suite's records. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("inner-records-contained", "An inner scenario's records never appear in the outer suite's records", inner::inner_records_contained),
        // In a boot session, an inner scenario and the real-runner fixture alike, a double-forked descendant that exits with status 7 after its parent has died is reparented to that session's supervisor (its parent pid read from `/proc/<pid>/stat` is the supervisor's) and reaped there, and the supervisor reports its pid and status 7 to the case; when the supervisor is killed first, the orphan goes to the runner, the runner writes `artifacts=none:supervisor-died`, and the case fails naming the dead supervisor. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("subreaper", "A double-forked orphan is reparented to and reaped by its session supervisor, or by the runner when the supervisor died first", sessions::subreaper).session(libuitest::supervisor::serve),
    ]),
    category("input", "Scripted keyboard, pointer & text input", &[
        // type_text("hello world") puts exactly that text in the focused text field. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("type-ascii", "type_text(\"hello world\") puts exactly that text in the focused text field", || pending("input", 5)),
        // Characters that need Shift arrive correctly. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("type-shifted", "Characters that need Shift arrive correctly", || pending("input", 5)),
        // Every printable ASCII character, typed through libuitest's own literal key table, arrives exactly. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("type-keymap-all", "Every printable ASCII character, typed through libuitest's literal key table, arrives exactly", || pending("input", 5)),
        // Characters outside the keymap arrive through a text event. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("type-utf8", "Characters outside the keymap arrive through a text event", || pending("input", 5)),
        // Newline and Tab arrive as the Enter and Tab keys. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("type-control", "Newline and Tab arrive as the Enter and Tab keys", || pending("input", 5)),
        // chord(Ctrl, A) arrives as A with the Ctrl modifier. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("chord", "chord(Ctrl, A) arrives as A with the Ctrl modifier", || pending("input", 5)),
        // Holding a key for 600 ms of UI time gives exactly 3 repeats. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("hold-repeat", "Holding a key for 600 ms of UI time gives exactly 3 repeats", || pending("input", 6)),
        // click(node) lands on a visible point of the node, and the delivery names that node's window. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("click-node", "click(node) lands on a visible point of the node, and the delivery names that node's window", || pending("input", 5)),
        // Clicking a node partly covered by another window lands on its exposed part, confirmed by hit test. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("click-partly-covered", "Clicking a node partly covered by another window lands on its exposed part, confirmed by hit test", || pending("input", 5)),
        // Clicking a node fully covered by another window fails the step by name. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("click-covered-fails", "Clicking a node fully covered by another window fails the step by name", || pending("input", 5)),
        // Clicking a node clipped by a scrolled container uses its visible part. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("click-clipped", "Clicking a node clipped by a scrolled container uses its visible part", || pending("input", 5)),
        // Clicking a node with no visible area fails the step by name. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("click-zero-area-fails", "Clicking a node with no visible area fails the step by name", || pending("input", 5)),
        // When the window moves between the query and the click, the click is rejected for a stale scene, re-resolved and lands correctly. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("click-stale-scene", "A window move between query and click is rejected as a stale scene, re-resolved and lands correctly", || pending("input", 7)),
        // When the node's list scrolls inside an unmoved window between the query and the click, Breengel rejects the unstarted click for a stale layout, and the re-resolved click lands on the node; when the press itself expands the list, the release still reaches the pressed node through capture, the click activates exactly once, nothing is retried, and no node is left pressed; when the pressed node is removed before the release, the gesture is cancelled, its release is `client-rejected(gesture-cancelled)`, and the step fails by name without a retry. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("click-stale-layout", "An unstarted click hit by a layout change is rejected and re-resolved; a started gesture is captured or canceled, never retried", || pending("input", 7)),
        // double_click delivers click counts 1 and 2. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("double-click", "double_click delivers click counts 1 and 2", || pending("input", 6)),
        // A ten-step drag delivers ten motions in order between a press and a release; when the layout changes on the press and again mid-drag, and when the window moves mid-drag, every later motion and the release still reach the captured node through bwm's grab, the drag ends in exactly one completed gesture, and no button, pressed flag or capture is held at the end. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("drag-steps", "A ten-step drag reaches its captured node in order through layout changes and a window move, ending with nothing held", || pending("input", 7)),
        // scroll(node, dy) delivers the wheel amount to the window under the node. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("scroll-node", "scroll(node, dy) delivers the wheel amount to the window under the node", || pending("input", 5)),
        // Typing into a node focuses it first, and a node that cannot take focus fails by name. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("focus-then-type", "Typing into a node focuses it first, and a node that cannot take focus fails by name", || pending("input", 5)),
        // delivered(serial) returns only after the target window processed that serial. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("delivered", "delivered(serial) returns only after the target window processed that serial", || pending("input", 5)),
        // An event sent to a hung app is reported unprocessed when the 2 s settle bound passes, naming the serial and the window. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("undelivered-fails", "An event sent to a hung app is reported unprocessed at the 2 s settle bound, naming the serial and window", || pending("input", 6)),
        // Every serial of a script ends with a terminal disposition. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("dispositions-all", "Every serial of a script ends with a terminal disposition", || pending("input", 5)),
        // The input log records each event at the core, the WM and the client. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("trace-hops", "The input log records each event at the core, the WM and the client", || pending("input", 5)),
        // Injected events are tagged synthetic in the trace. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("synthetic-tagged", "Injected events are tagged synthetic in the trace", || pending("input", 5)),
        // A non-root session cannot inject, after a root session has. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("nonroot-denied", "A non-root session cannot inject, after a root session has", || pending("input", 5)),
        // Ending a session releases every held key and button. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("teardown-releases", "Ending a session releases every held key and button", sessions::teardown_releases).session(libuitest::supervisor::serve),
        // Input arrives in order while the app animates. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("input-during-animation", "Input arrives in order while the app animates", || pending("input", 6)),
        // Input addressed to two windows of one app arrives at each. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("two-windows", "Input addressed to two windows of one app arrives at each", || pending("input", 5)),
        // A scripted Super double tap opens the launcher through bwm's real hotkey path. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("wm-hotkey-scripted", "A scripted Super double tap opens the launcher through bwm's real hotkey path", || pending("input", 5)),
        // Events from a device outside the session's selection, modifier keys included, change nothing in the session. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("real-input-excluded", "Events from a device outside the session's selection, modifier keys included, change nothing in the session", || pending("input", 5)),
    ]),
    category("tree", "Semantic trees of windows & widgets", &[
        // The test app's tree lists every widget with its expected role and key. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("roles-keys", "The test app's tree lists every widget with its expected role and key", || pending("tree", 5)),
        // Names and values match the widgets' states set by the script. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("names-values", "Names and values match the widgets' states set by the script", || pending("tree", 5)),
        // Menu items lie inside their menu, tabs inside their tablist, and thumbs inside their track. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("role-invariants", "Menu items lie inside their menu, tabs inside their tablist, and thumbs inside their track", || pending("tree", 5)),
        // The focused flag moves with Tab. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("focus-flag", "The focused flag moves with Tab", || pending("tree", 5)),
        // The focused widget draws the theme's focus ring at its bounds. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("focus-ring-pixels", "The focused widget draws the theme's focus ring at its bounds", || pending("tree", 5)),
        // A disabled button is flagged not enabled. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("enabled-flag", "A disabled button is flagged not enabled", || pending("tree", 5)),
        // Checked, selected and pressed flags follow interaction. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("state-flags", "Checked, selected and pressed flags follow interaction", || pending("tree", 5)),
        // A text field's value equals the typed text. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("textfield-value", "A text field's value equals the typed text", || pending("tree", 5)),
        // A terminal node's rows, cell attributes and cursor equal what its grid holds. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("terminal-grid", "A terminal node's rows, cell attributes and cursor equal what its grid holds", || pending("tree", 5)),
        // A tree and its pixels always come from the same commit. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("commit-coupled", "A tree and its pixels always come from the same commit", || pending("tree", 5)),
        // Over 1,000 rapid commits, every read gives a tree and pixels from one commit. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("rapid-commits", "Over 1,000 rapid commits, every read gives a tree and pixels from one commit", || pending("tree", 5)),
        // A tree larger than its page is flagged truncated, not corrupted. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("truncation-flagged", "A tree larger than its page is flagged truncated, not corrupted", || pending("tree", 5)),
        // bwm's tree lists each window's title, bounds, z order, focus and minimized state. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("wm-tree", "bwm's tree lists each window's title, bounds, z order, focus and minimized state", || pending("tree", 5)),
        // bwm's tree agrees with the operation model and the client logs after every step of a script. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("wm-tree-vs-model", "bwm's tree agrees with the operation model and the client logs after every step of a script", || pending("tree", 5)),
        // A query that matches exactly one node returns it. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("find-unique", "A query that matches exactly one node returns it", || pending("tree", 5)),
        // A query that matches two nodes fails, naming both. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("find-ambiguous-fails", "A query that matches two nodes fails, naming both", || pending("tree", 5)),
        // A query that matches nothing fails, naming the query. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("find-none-fails", "A query that matches nothing fails, naming the query", || pending("tree", 5)),
        // Nodes scrolled out or covered are flagged not visible, with their clip. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("hidden-nodes", "Nodes scrolled out or covered are flagged not visible, with their clip", || pending("tree", 5)),
        // Scrolling a list changes which items are flagged visible. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("list-scroll", "Scrolling a list changes which items are flagged visible", || pending("tree", 5)),
        // An unrelated non-root process cannot map another app's tree. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("permission", "An unrelated non-root process cannot map another app's tree", || pending("tree", 5)),
        // With no reader attached the tree is never serialized, and with one it is serialized once per commit. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("no-reader-no-serialize", "With no reader attached the tree is never serialized, and with one it is serialized once per commit", || pending("tree", 5)),
        // The text dump of a tree parses back to the same tree. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("dump-roundtrip", "The text dump of a tree parses back to the same tree", || pending("tree", 5)),
        // A label whose tree text is "PASS" is checked against the certified renderer's "PASS" at its bounds. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("text-pixels", "A label whose tree text is PASS is checked against the certified renderer's PASS at its bounds", || pending("tree", 5)),
    ]),
    category("frames", "Frame capture & comparison", &[
        // A window capture equals the committed buffer of that commit. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("window-capture", "A window capture equals the committed buffer of that commit", || pending("frames", 4)),
        // Capturing commit N while the app draws N+1 returns N whole. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("capture-while-drawing", "Capturing commit N while the app draws N+1 returns N whole", || pending("frames", 4)),
        // Two readers capturing the same window each get whole commits. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("capture-two-readers", "Two readers capturing the same window each get whole commits", || pending("frames", 4)),
        // A resize during a capture gives the whole old commit, pixels and tree, never ENOENT or a mixture. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("capture-resize-race", "A resize during a capture gives the whole old commit, pixels and tree, never ENOENT or a mixture", || pending("frames", 4)),
        // A destroy during a capture gives the whole commit and frees the buffer afterwards; a destroy before the copy begins gives ENOENT. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("capture-destroy-race", "A destroy during a capture gives the whole commit and frees the buffer afterwards; a destroy before the copy begins gives ENOENT", || pending("frames", 4)),
        // Commits from two producer threads are each captured whole. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("capture-threads", "Commits from two producer threads are each captured whole", || pending("frames", 4)),
        // A screen capture shows the window at its position, with its observation record. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("screen-capture", "A screen capture shows the window at its position, with its observation record", || pending("frames", 4)),
        // A screen capture lists the commit of each window it contains. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("capture-commits", "A screen capture lists the commit of each window it contains", || pending("frames", 4)),
        // An identical frame matches with zero differing pixels. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("exact-match", "An identical frame matches with zero differing pixels", || pending("frames", 4)),
        // A frame with one changed pixel fails as pixel-mismatch, naming that pixel and both colours. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("one-pixel-fails", "A frame with one changed pixel fails as pixel-mismatch, naming that pixel and both colors", || pending("frames", 4)),
        // A difference of 2 passes at tolerance 2 and fails at tolerance 1. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("channel-tolerance", "A difference of 2 passes at tolerance 2 and fails at tolerance 1", || pending("frames", 4)),
        // The differing-pixel cap passes at its limit and fails one pixel past it. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("max-differing", "The differing-pixel cap passes at its limit and fails one pixel past it", || pending("frames", 4)),
        // All regions of a check must pass in the same frame. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("regions-same-frame", "All regions of a check must pass in the same frame", || pending("frames", 4)),
        // A masked region is excluded, and a mask without a reason fails validation. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("mask", "A masked region is excluded, and a mask without a reason fails validation", frames::mask),
        // Masks that leave less than 75% of a region fail validation; the 100x100 region whose four one-pixel border strips are masked (396 pixels, 96.04% still compared) passes this region rule, so `structural-vacuous` must be what rejects it. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("mask-cap", "Masks leaving under 75% of a region fail validation; four masked one-pixel border strips (96.04% compared) pass this rule", frames::mask_cap),
        // A check carrying a field schema version 1 does not define, `edgeRadius` included, fails validation. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("unknown-field", "A check carrying a field schema version 1 does not define, edgeRadius included, fails validation", frames::unknown_field),
        // Frames of different sizes fail without comparing. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("size-mismatch", "Frames of different sizes fail without comparing", || pending("frames", 4)),
        // A reference whose SHA-256 differs from its pin fails before comparing. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("reference-pin", "A reference whose SHA-256 differs from its pin fails before comparing", || pending("frames", 4)),
        // A missing reference fails, naming the path. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("reference-missing", "A missing reference fails, naming the path", || pending("frames", 4)),
        // A formula check compares like a stored one. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("formula", "A formula check compares like a stored one", || pending("frames", 4)),
        // A uniform-region check passes on a flat fill and fails on one stray pixel. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("structural-uniform", "A uniform-region check passes on a flat fill and fails on one stray pixel", || pending("frames", 4)),
        // A no-colour-outside check catches a pixel leaking past its rectangle. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("structural-outside", "A no-color-outside check catches a pixel leaking past its rectangle", || pending("frames", 4)),
        // A colour-count check catches a blank frame. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("structural-count", "A color-count check catches a blank frame", || pending("frames", 4)),
        // Each predicate's effective-domain rule is enforced after masks, before any comparison: a uniform check without a colour, a colour count below 2 or above the compared pixels, overlapping equal regions, equal regions with fewer than 2 compared pairs, `minDiffering` above the domain, an operand that names no declared region, and a region of one pixel each fail validation; and an `outside` check on a 100x100 region with a strictly interior 98x98 `rect` and its four border strips masked fails validation for an empty outside domain, while the same check unmasked catches one forbidden-colour pixel planted in the border. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("structural-vacuous", "Every structural predicate's effective-domain rule is enforced after masks, before any comparison", frames::structural_vacuous),
        // A formula check with a missing, extra, mistyped or out-of-range parameter fails validation. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("formula-params", "A formula check with a missing, extra, mistyped or out-of-range parameter fails validation", frames::formula_params),
        // A check with an unknown schema version fails validation. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("schema-version", "A check with an unknown schema version fails validation", frames::schema_version),
        // A check bound to a window key or checkpoint the scenario never reaches fails, naming it. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("schema-binding", "A check bound to a window key or checkpoint the scenario never reaches fails, naming it", || pending("frames", 4)),
        // A reference PNG decodes to the pixels that were encoded. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("png-reference", "A reference PNG decodes to the pixels that were encoded", || pending("frames", 4)),
        // Captures taken with the cursor hidden contain no cursor pixels. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("cursor-excluded", "Captures taken with the cursor hidden contain no cursor pixels", || pending("frames", 4)),
        // The test app's canonical scene hashes to the pinned value on this target. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("cross-target", "The test app's canonical scene hashes to the pinned value on this target", || pending("frames", 4)),
        // Every check writes one well-formed FRAME record with its definition hash. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("frame-record", "Every check writes one well-formed FRAME record with its definition hash", || pending("frames", 4)),
        // Comparing two 1280x800 frames takes at most 50 ms. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("compare-cost", "Comparing two 1280x800 frames takes at most 50 ms", || pending("frames", 4)),
    ]),
    category("clock", "UI time & stepping", &[
        // An app reading the UI clock sees it stand still between steps. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("clock-frozen", "An app reading the UI clock sees it stand still between steps", || pending("clock", 6)),
        // step(16) advances the app's clock by exactly 16 ms and the epoch by one. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("clock-step", "step(16) advances the app's clock by exactly 16 ms and the epoch by one", || pending("clock", 6)),
        // Each step is acknowledged by every required participant in its commit metadata. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("epochs-acked", "Each step is acknowledged by every required participant in its commit metadata", || pending("clock", 6)),
        // Two timers due at the same time fire in registration order. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("equal-deadlines", "Two timers due at the same time fire in registration order", || pending("clock", 6)),
        // A periodic timer whose period passes ten times in one step fires once and reports nine missed. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("multiple-expiry", "A periodic timer whose period passes ten times in one step fires once and reports nine missed", || pending("clock", 6)),
        // step(0) advances the epoch, fires only timers due now, and is acknowledged without new frames. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("zero-step", "step(0) advances the epoch, fires only timers due now, and is acknowledged without new frames", || pending("clock", 6)),
        // One step of 1,000 ms leaves an animation in the same state as a thousand steps of 1 ms. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("large-step", "One step of 1,000 ms leaves an animation in the same state as a thousand steps of 1 ms", || pending("clock", 6)),
        // Each commit carries its epoch and its UI time in milliseconds. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("stamps", "Each commit carries its epoch and its UI time in milliseconds", || pending("clock", 6)),
        // An app's one-second timer fires on step(1000), not on real time. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("timers-virtual", "An app's one-second timer fires on step(1000), not on real time", || pending("clock", 6)),
        // Wall-clock time shown by apps and bwm is the fixed instant plus elapsed UI time. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("wall-time-fixed", "Wall-clock time shown by apps and bwm is the fixed instant plus elapsed UI time", || pending("clock", 6)),
        // bwm's taskbar clock and repeat timer read the UI clock. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("wm-clock", "bwm's taskbar clock and repeat timer read the UI clock", || pending("clock", 6)),
        // Two apps in one session step together. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("shared-clock", "Two apps in one session step together", || pending("clock", 6)),
        // A stopped participant makes step fail naming it, unless the case excluded it. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("stopped-unacked-fails", "A stopped participant makes step fail naming it, unless the case excluded it", || pending("clock", 6)),
        // Stepping never deadlocks against buffer back-pressure. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("backpressure-stepped", "Stepping never deadlocks against buffer back-pressure", || pending("clock", 6)),
        // An app that animates forever still lets every step complete. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("continuous-animation", "An app that animates forever still lets every step complete", || pending("clock", 6)),
        // An app's real-clock read counter stays 0 in a session and rises when the test app reads the real clock on purpose. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("real-clock-counter", "An app's real-clock read counter stays 0 in a session and rises when the test app reads the real clock on purpose", || pending("clock", 6)),
        // Injected events carry UI time, and bwm's double-click threshold uses it. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("input-time", "Injected events carry UI time, and bwm's double-click threshold uses it", || pending("clock", 6)),
        // With the UI clock frozen, the case is still ended by its real deadline. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("watchdog-real", "With the UI clock frozen, the case is still ended by its real deadline", || pending("clock", 6)),
        // With a UI clock set, Breengel's clock functions never read the real clocks. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("no-real-clock", "With a UI clock set, Breengel's clock functions never read the real clocks", || pending("clock", 6)),
    ]),
    category("settle", "Causal barriers & settling", &[
        // settle returns once nothing changes. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("idle", "settle returns once nothing changes", || pending("settle", 6)),
        // settle waits until the target window commits with the serial processed, not until it reads the event. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("processed-not-consumed", "settle waits until the target window commits with the serial processed, not until it reads the event", || pending("settle", 6)),
        // An app that drains events and never commits makes settle fail, naming the window and the serial. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("drained-not-rendered", "An app that drains events and never commits makes settle fail, naming the window and the serial", || pending("settle", 6)),
        // An event that changes nothing settles through commit_no_change. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("no-change-ack", "An event that changes nothing settles through commit_no_change", || pending("settle", 6)),
        // settle waits until a completed composite lists every visible window at its newest pixel generation. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("composited", "settle waits until a completed composite lists every visible window at its newest pixel generation", || pending("settle", 6)),
        // Settling after a commit_no_change completes through the composite that already holds the window's pixels, with no new composite. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("no-change-no-composite", "Settling after a commit_no_change completes through the composite that already holds the window's pixels", || pending("settle", 6)),
        // An idle step settles through bwm's `none-needed` acknowledgment, with the composite counter unchanged. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("bwm-ack-no-composite", "An idle step settles through bwm's none-needed acknowledgment, with the composite counter unchanged", || pending("settle", 6)),
        // A hotkey settles with no client involved, through its WM-handled disposition. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("wm-handled", "A hotkey settles with no client involved, through its WM-handled disposition", || pending("settle", 6)),
        // settle waits for a named tree predicate on an asynchronous result. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("async-predicate", "settle waits for a named tree predicate on an asynchronous result", || pending("settle", 6)),
        // A settle that cannot complete fails, naming the window, serial or predicate it waited for. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("timeout-names", "A settle that cannot complete fails, naming the window, serial or predicate it waited for", || pending("settle", 6)),
        // settle returns while an app has a future animation deadline. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("animation-allowed", "settle returns while an app has a future animation deadline", || pending("settle", 6)),
        // A minimized window still acknowledges epochs and processes its serials, settle waits for those, and settle does not wait for its pixels; once restored, the next settle waits for its current generation. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("minimized-acks", "A minimized window acknowledges and processes without being waited on for pixels until it is restored", || pending("settle", 6)),
        // Repeated settle calls with nothing changing cause no extra frames. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("repeated-settle", "Repeated settle calls with nothing changing cause no extra frames", || pending("settle", 6)),
    ]),
    category("sessions", "Sessions, state isolation & cleanup", &[
        // A desktop session with one app reaches its first composited commit within 300 ms. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("start-desktop", "A desktop session with one app reaches its first composited commit within 300 ms", sessions::start_desktop).session(libuitest::supervisor::serve).collects(),
        // A kiosk session with one app reaches its first composited commit within 300 ms. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("start-kiosk", "A kiosk session with one app reaches its first composited commit within 300 ms", sessions::start_kiosk).session(libuitest::supervisor::serve).collects(),
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
        // The panel is redrawn after the case, with that case's result. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("panel-after-case", "The panel is redrawn after the case, with that case's result", sessions::panel_after_case),
        // When a session's compositor dies, the display goes to the session supervisor, then to the runner at case end. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("ownership-stack", "When a session's compositor dies, the display goes to the session supervisor, then to the runner at case end", sessions::ownership_stack),
        // A scenario that runs out of time fails, and its evidence is saved inside the reserve. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("case-timeout", "A scenario that runs out of time fails, and its evidence is saved inside the reserve", sessions::case_timeout),
        // An app crashing during a scenario fails it, naming the app and the signal. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("app-crash", "An app crashing during a scenario fails it, naming the app and the signal", sessions::app_crash),
        // An app's stderr is saved, capped at 64 KiB. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("app-stderr-capped", "An app's stderr is saved, capped at 64 KiB", sessions::app_stderr_capped).session(libuitest::supervisor::serve),
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
        // A GUI suite runs inside a suite sequence and hands off at READY. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("sequence-compatible", "A GUI suite runs inside a suite sequence and hands off at READY", sessions::sequence_compatible),
        // The session's fixture tree, fonts and locale match their pinned hashes. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("fixtures-pinned", "The session's fixture tree, fonts and locale match their pinned hashes", sessions::fixtures_pinned).session(libuitest::supervisor::serve),
    ]),
    category("diagnosis", "Saved evidence & failure messages", &[
        // A mismatch saves the got image, diff image, tree, input log and screen. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("artifacts-on-mismatch", "A mismatch saves the got image, diff image, tree, input log and screen", || pending("diagnosis", 7)),
        // A passing case saves nothing unless saving is turned on. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("nothing-on-pass", "A passing case saves nothing unless saving is turned on", || pending("diagnosis", 7)),
        // With saving turned on, a passing case saves its frames. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("save-all", "With saving turned on, a passing case saves its frames", || pending("diagnosis", 7)),
        // The diff image marks exactly the differing pixels. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("diff-image", "The diff image marks exactly the differing pixels", || pending("diagnosis", 7)),
        // The saved got image decodes to the captured frame. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("got-image", "The saved got image decodes to the captured frame", || pending("diagnosis", 7)),
        // The saved tree is the one from the compared commit: under the real runner in the fixture, a case whose window check fails is killed before it writes any artifact, after the app has committed 30 more frames with different trees, and the app is then killed; the saved tree matches the compared commit's id and tree hash from the arena record, not the app's last commit or its remnant. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("tree-at-commit", "The saved tree is the compared commit's, after 30 later commits, the case's death before writing and the app's death", || pending("diagnosis", 7)),
        // The saved input log lists every serial with its disposition. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("input-log", "The saved input log lists every serial with its disposition", || pending("diagnosis", 7)),
        // The saved screen is the readback at the failing step, with its record. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("screen-saved", "The saved screen is the readback at the failing step, with its record", || pending("diagnosis", 7)),
        // Past the artifact cap, a failure records artifacts=none:cap and keeps its primary message. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("cap", "Past the artifact cap, a failure records artifacts=none:cap and keeps its primary message", || pending("diagnosis", 7)),
        // An artifact write that fails records artifacts=none:io, and the primary failure stands. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("io-error", "An artifact write that fails records artifacts=none:io, and the primary failure stands", || pending("diagnosis", 7)),
        // The FRAME record says what was saved and where. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("path-in-record", "The FRAME record says what was saved and where", || pending("diagnosis", 7)),
        // Artifacts are on disk before the CASE line is written. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("synced", "Artifacts are on disk before the CASE line is written", || pending("diagnosis", 7)),
        // A frame failure's message names the check, the first pixel and both colours. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("message-pixel", "A frame failure's message names the check, the first pixel and both colors", || pending("diagnosis", 7)),
        // A tree failure's message names the node key and the field. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("message-tree", "A tree failure's message names the node key and the field", || pending("diagnosis", 7)),
        // A delivery failure's message names the serial and its disposition. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("message-input", "A delivery failure's message names the serial and its disposition", || pending("diagnosis", 7)),
        // When the case process is killed, the runner sends `end collect`, waits for the supervisor's `done` before sweeping, and the evidence is on disk. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("case-death", "When the case process is killed, the runner sends end collect, waits for done before sweeping, and the evidence is on disk", || pending("diagnosis", 7)),
        // The screen, frame and tree saved for a failing check are the ones captured at the failure, after 30 later commits and composites, the case's death before it wrote artifacts, and the window owner's death; a slot left `writing` by the killed case is recorded `partial:case-died` and never saved as evidence. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("failed-screen-retained", "The screen, frame and tree saved for a failing check are those captured at the failure; a slot left writing is recorded partial:case-died", || pending("diagnosis", 7)),
        // An app that dies right after a commit still has that commit and tree saved, from its remnant. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("app-death", "An app that dies right after a commit still has that commit and tree saved, from its remnant", || pending("diagnosis", 7)),
        // Evidence collection finishes inside the reserve, before the runner's limit. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("reserve", "Evidence collection finishes inside the reserve, before the runner's limit", || pending("diagnosis", 7)),
    ]),
    category("proof", "Proven on bwm & a test app", &[
        // The test app with no fault passes every check of a full script. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("clean-passes", "The test app with no fault passes every check of a full script", || pending("proof", 8)),
        // A frame drawn one pixel off fails as pixel-mismatch at the first shifted pixel. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("fault-off-by-one", "A frame drawn one pixel off fails as pixel-mismatch at the first shifted pixel", || pending("proof", 8)),
        // A widget drawn in the wrong color fails as pixel-mismatch with both colors. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("fault-wrong-color", "A widget drawn in the wrong color fails as pixel-mismatch with both colors", || pending("proof", 8)),
        // An app that drains a key and never commits fails as unprocessed-serial for that key. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("fault-drain-no-render", "An app that drains a key and never commits fails as unprocessed-serial for that key", || pending("proof", 8)),
        // An app that updates its tree but not its pixels fails as pixel-mismatch. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("fault-stale-pixels", "An app that updates its tree but not its pixels fails as pixel-mismatch", || pending("proof", 8)),
        // An app that drops one key fails as tree-mismatch on the text field's value. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("fault-drop-key", "An app that drops one key fails as tree-mismatch on the text field's value", || pending("proof", 8)),
        // bwm-fault_test sending keys to the wrong window fails as pixel-mismatch and tree-mismatch on the intended window. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("fault-misroute", "bwm-fault_test sending keys to the wrong window fails as pixel-mismatch and tree-mismatch on the intended window", || pending("proof", 8)),
        // bwm-fault_test drawing chrome one pixel off fails as pixel-mismatch against the contract table. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("fault-chrome-offset", "bwm-fault_test drawing chrome one pixel off fails as pixel-mismatch against the contract table", || pending("proof", 8)),
        // bwm-fault_test with two keys swapped fails as tree-mismatch on typed text. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("fault-keymap-swap", "bwm-fault_test with two keys swapped fails as tree-mismatch on typed text", || pending("proof", 8)),
        // A tree whose bounds disagree with the drawn widget, still inside its parent, fails as tree-mismatch against the literal expected geometry. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("fault-lie-bounds", "A tree whose bounds disagree with the drawn widget, still inside its parent, fails as tree-mismatch against the literal expected geometry", || pending("proof", 8)),
        // A label drawn "FAIL" whose tree says "PASS", with valid ink and bounds, fails as pixel-mismatch. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("fault-lie-text", "A label drawn FAIL whose tree says PASS, with valid ink and bounds, fails as pixel-mismatch", || pending("proof", 8)),
        // An app that never settles fails as settle-timeout, naming it. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("fault-slow-commit", "An app that never settles fails as settle-timeout, naming it", || pending("proof", 8)),
        // An app animating on the real clock fails as clock-violation on every run. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("fault-wallclock", "An app animating on the real clock fails as clock-violation on every run", || pending("proof", 8)),
        // A hung app fails as app-hung when the 2 s step bound passes, instead of hanging the suite. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("fault-hang", "A hung app fails as app-hung when the 2 s step bound passes, instead of hanging the suite", || pending("proof", 8)),
        // A crashing app fails as app-crashed, naming the signal. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("fault-crash", "A crashing app fails as app-crashed, naming the signal", || pending("proof", 8)),
        // A cursor trail fails as pixel-mismatch in a screen check. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("fault-trail", "A cursor trail fails as pixel-mismatch in a screen check", || pending("proof", 8)),
        // bwm-fault_test leaving a serial without a disposition fails as missing-disposition. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("fault-lost-disposition", "bwm-fault_test leaving a serial without a disposition fails as missing-disposition", || pending("proof", 8)),
        // bwm-fault_test that ignores the session's device selection fails when a non-selected device types. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("fault-real-input", "bwm-fault_test that ignores the session's device selection fails when a non-selected device types", || pending("proof", 8)),
        // Filling the test app's form (text field, checkbox, slider, submit) gives the expected tree and frame. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("form-scenario", "Filling the test app's form (text field, checkbox, slider, submit) gives the expected tree and frame", || pending("proof", 8)),
        // Opening a menu and choosing an item gives the expected tree and frame. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("menu-scenario", "Opening a menu and choosing an item gives the expected tree and frame", || pending("proof", 8)),
        // Scrolling and selecting in a list gives the expected tree and frame. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("list-scenario", "Scrolling and selecting in a list gives the expected tree and frame", || pending("proof", 8)),
        // Stepping the test app's animation gives its reference frames at epochs 1, 10 and 60. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("animation-scenario", "Stepping the test app's animation gives its reference frames at epochs 1, 10 and 60", || pending("proof", 8)),
        // A scripted focus scenario on bwm gives the expected trees and screens. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("wm-focus-scenario", "A scripted focus scenario on bwm gives the expected trees and screens", || pending("proof", 8)),
        // A scripted stacking scenario on bwm gives the expected trees and screens. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("wm-stack-scenario", "A scripted stacking scenario on bwm gives the expected trees and screens", || pending("proof", 8)),
        // A scripted resize of the test app gives the expected trees and screens. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("wm-resize-scenario", "A scripted resize of the test app gives the expected trees and screens", || pending("proof", 8)),
        // Copying between two test apps through bwm gives the expected text fields. (claim-lint:ok: docs/gui/plan.md section 17 specifies this future case.)
        case("wm-clipboard-scenario", "Copying between two test apps through bwm gives the expected text fields", || pending("proof", 8)),
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
