// E18: real bwm, independent GPU clients and device-backed scanout observation.
#[path = "wm_suite/clipboard.rs"]
mod clipboard;
#[path = "wm_suite/composition.rs"]
mod composition;
#[path = "wm_suite/focus.rs"]
mod focus;
#[path = "wm_suite/geometry.rs"]
mod geometry;
#[path = "wm_suite/input_core.rs"]
mod input_core;
#[path = "wm_suite/input_devices.rs"]
mod input_devices;
#[path = "wm_suite/kit.rs"]
mod kit;
#[path = "wm_suite/lifecycle.rs"]
mod lifecycle;
#[path = "wm_suite/liveness.rs"]
mod liveness;
#[path = "wm_suite/performance.rs"]
mod performance;
#[path = "wm_suite/pointer.rs"]
mod pointer;
#[path = "wm_suite/recovery.rs"]
mod recovery;
#[path = "wm_suite/stacking.rs"]
mod stacking;
#[path = "wm_suite/startup.rs"]
mod startup;

#[path = "wm_suite/desktop.rs"]
pub mod desktop;
#[path = "wm_suite/event_log.rs"]
pub mod event_log;
#[path = "wm_suite/input.rs"]
mod input;
#[path = "wm_suite/scene.rs"]
mod scene;
#[path = "wm_suite/measure.rs"]
mod measure;
#[path = "wm_suite/protocol.rs"]
pub mod protocol;
use breenix_suite::suite::{case, category, suite, Suite};

static SUITE: Suite = suite("window-manager", "Window manager", &[
    category("input-core", "Input core & virtual devices", &[
        case("virtual-keyboard", "A virtual A press and release traverse the core and bwm to a real client", input_core::virtual_keyboard),
        case("serials", "Input serials increase across two virtual devices and bwm delivers both", input_core::core_serials),
        case("time-injected", "Injector timestamps survive routing and decreasing time is refused", input_core::core_timestamps),
        case("ctrl-super-distinct", "Virtual Ctrl+A reaches a client with Ctrl and without Super", input_core::core_modifiers),
        case("text-event", "Whole UTF-8 EV_TEXT reaches a client and invalid UTF-8 is refused", input_core::core_text),
        case("device-removal", "Removing a held virtual keyboard sends the release to its client", input_core::core_removal),
        case("virtual-pointer", "A virtual absolute pointer and button traverse the core and bwm at exact client coordinates", input_core::virtual_pointer),
        case("ledger-complete", "A mixed virtual script has one terminal disposition per serial and delivered keys have processing commits", input_core::core_ledger),
        case("capabilities", "Undeclared keys return EINVAL between two successful declared-key deliveries", input_core::core_capabilities),
        case("uinput-create", "Root creates a virtual keyboard and pointer device with declared capabilities.", input_core::uinput_create),
        case("uinput-nonroot", "A non-root open of /dev/uinput fails with EACCES after root's open has succeeded.", input_core::uinput_nonroot),
        case("abi-layout", "The event struct has the contract's literal size and field offsets.", input_core::abi_layout),
        case("key-press-release", "A virtual key press and release reach the event stream as values 1 and 0 with Linux keycodes.", input_core::key_press_release),
        case("modifiers-derived", "Shift, Ctrl, Alt and Super state follows key events, the same for every device.", input_core::modifiers_derived),
        case("pointer-absolute", "An absolute event maps to the screen coordinates the contract's formula gives.", input_core::pointer_absolute),
        case("pointer-relative", "Relative motion accumulates and clamps to the screen.", input_core::pointer_relative),
        case("buttons", "Left, right and middle press and release arrive as sent.", input_core::buttons),
        case("wheel", "Vertical and horizontal wheel steps arrive with their signs.", input_core::wheel),
        case("syn-batches", "Events between SYN_REPORTs are delivered as one batch.", input_core::syn_batches),
        case("time-real", "A real-device event carries CLOCK_MONOTONIC time.", input_core::time_real),
        case("two-readers", "Two readers of the event stream each receive every event.", input_core::two_readers),
        case("syn-dropped", "A reader that falls behind receives SYN_DROPPED, and each dropped serial is recorded as lost.", input_core::syn_dropped),
        case("poll", "The event device reports POLLIN through poll when events are queued.", input_core::poll),
        case("grab", "While the compositor holds the grab, the console sees no key events except the reserved console chords, Ctrl+Alt+F1 to F12 (`docs/console/contract.md` section 9), which go to the console and which the grab holder sees neither pressed nor released.", input_core::grab),
        case("grab-select", "Events from a device outside the session's selection reach no client and are recorded as not-selected.", input_core::grab_select),
        case("grab-drain", "Taking the grab drains queued events and resets pressed state.", input_core::grab_drain),
        case("text-invalid", "Invalid UTF-8, a truncated record and an oversize payload are each rejected whole.", input_core::text_invalid),
        case("malformed-batch", "A truncated batch or an unknown type is rejected with EINVAL, and nothing of it is published.", input_core::malformed_batch),
        case("console-keymap", "With no compositor, key events become the console bytes the US keymap gives.", input_core::console_keymap),
        case("console-special-keys", "Arrows, Home, End, Delete, Page Up and Page Down become VT sequences on the console.", input_core::console_special_keys),
        case("console-modifiers", "Ctrl+C on the console gives byte 3 from every device type.", input_core::console_modifiers),
        case("ledger-gap", "A serial left without a disposition is reported as the first missing serial.", input_core::ledger_gap),
        case("console-after-compositor", "When the compositor dies, keys return to the console.", input_core::console_after_compositor),
        case("library-batches", "The public reader and injector preserve UTF-8 payloads, injector time and atomic serial allocation.", input_core::library_batches),
        case("reader-lifetime", "The compositor grab survives unrelated closes and dup, and ends on the last reader reference.", input_core::reader_lifetime),
    ]),
    category("input-devices", "Keyboard & pointer drivers on every target", &[
        case("keyboard-completion-path", "The bound keyboard completion reports, wakes bwm, delivers A and rearms", input_devices::keyboard_completion),
        case("pointer-completion-path", "The bound pointer completion reports, wakes bwm, delivers motion and rearms", input_devices::pointer_completion),
        case("replay-report-fault", "Suppressing the production report loses the event after a positive delivery", input_devices::replay_faults),
        case("hid-keyboard-decode", "Literal USB boot-keyboard reports decode to the literal expected events.", input_devices::hid_keyboard_decode),
        case("hid-rollover", "Six-key rollover and phantom-state reports decode as the contract says.", input_devices::hid_rollover),
        case("hid-relative", "Literal relative-mouse reports decode to relative motion.", input_devices::hid_relative),
        case("hid-wheel", "Literal wheel reports decode to wheel events.", input_devices::hid_wheel),
        case("virtio-keyboard-decode", "Literal virtio-input key events decode to the same core events.", input_devices::virtio_keyboard_decode),
        case("virtio-tablet-decode", "Literal virtio-input absolute, button and wheel events decode correctly.", input_devices::virtio_tablet_decode),
        case("replay-wake-fault", "With the wake skipped, the idle compositor's wake is missing, and the case says so.", input_devices::replay_wake_fault),
        case("devices-bound", "This target binds exactly the keyboard and pointer drivers its row of the coverage map names, by name.", input_devices::devices_bound),
        case("device-capabilities", "Each bound device reports the capabilities its driver declares.", input_devices::device_capabilities),
    ]),
    category("startup", "bwm starts and shows clients", &[
        case("desktop-control", "A completed empty desktop exposes the literal taskbar pixel", startup::desktop_control),
        case("claims-display", "A live bwm retains the compositor claim after composing a client", startup::claims_display),
        case("first-client", "A real client pattern and commit appear in completed device scanout", startup::first_client),
        case("existing-windows", "bwm adopts and composes a window created before it starts", startup::existing_windows),
        case("independent-clients", "Two independent owners share the desktop and the top client is exact", startup::independent_clients),
        case("supervisor-starts", "Production init desktop starts bwm and composes an independent client", startup::supervisor_starts),
        case("no-gpu-required", "bwm runs on this target's backend without exiting.", startup::no_gpu_required),
        case("epoch-ack", "bwm acknowledges each stepped UI-clock epoch in its status page, with `none-needed` and no composite when nothing it draws changed.", startup::epoch_ack),
        case("empty-desktop", "With no clients, the screen shows exactly the background, taskbar and appbar from the contract's table.", startup::empty_desktop),
        case("config-test", "The session bwm.conf sets pinned fonts, frozen clock text and no frame-rate output.", startup::config_test),
        case("config-invalid", "A malformed bwm.conf falls back to defaults and shows its error on the desktop.", startup::config_invalid),
        case("fonts-loaded", "bwm's configured UI fonts are used on every target.", startup::fonts_loaded),
        case("hotkeys-loaded", "hotkeys.conf bindings are read on every target.", startup::hotkeys_loaded),
        case("composited-marker", "bwm prints its composited-client marker only after a composite record lists a client commit.", startup::composited_marker),
    ]),
    category("composition", "Composition, chrome & repaint", &[
        case("chrome-geometry", "Decorations and desktop bars occupy the literal rectangles and colours", composition::chrome),
        case("overlap", "The higher client covers the lower client with its exact complete pattern", composition::overlap),
        case("cascade", "New windows cascade by 50 logical pixels on each axis", composition::cascade),
        case("client-update", "A new client commit replaces its pattern in completed device scanout", composition::client_update),
        case("expose-close", "An exited top client reveals the exact complete lower client", composition::expose_close),
        case("oracle-corruption", "The scanout check rejects a wrong content pixel and missing commit provenance", composition::frame_integrity),
        case("client-position", "A client's pattern appears at the content origin bwm assigned, exactly.", composition::client_position),
        case("title-text", "The window's title matches the certified text renderer's drawing at the contract's position.", composition::title_text),
        case("shadow", "The drop shadow's pixels match the contract's formula.", composition::shadow),
        case("background", "The desktop background shows wherever no window is.", composition::background_pixels),
        case("appbar-entries", "The appbar lists one entry per window, in creation order.", composition::appbar_entries),
        case("chromeless", "A chromeless window has no frame, sits where the contract says and has the dimmer behind it.", composition::chromeless),
        case("sixteen-clients", "Sixteen clients each show their own pattern.", composition::sixteen_clients),
        case("moving-client", "An animating client's every committed frame is composed in order as the case steps it.", composition::moving_client),
        case("frozen-client", "A hung client keeps its last frame on screen while the chrome stays live.", composition::frozen_client),
        case("present-failure", "A forced composite failure is reported by bwm and retried, and is not counted as a frame.", composition::present_failure),
        case("visible-only", "A minimized window's content is left out of composition.", composition::visible_only),
        case("expose-move", "Moving a window repaints the uncovered area exactly.", composition::expose_move),
        case("partial-equals-full", "A partial redraw gives the same screen as a full redraw.", composition::partial_equals_full),
        case("damage-bounded", "Moving a small window composes exactly the union of its old and new outer painted bounds.", composition::damage_bounded),
        case("outer-bounds", "The outer painted bounds, shadow and cursor included, match the contract's table.", composition::outer_bounds),
        case("scene-identical", "A fixed three-window scene hashes to the pinned value, the same on every target.", composition::scene_identical),
        case("resized-content", "After a resize, the content area shows the client's committed buffer at its new size.", composition::resized_content),
        case("cursor-over-window", "The cursor draws above windows and leaves no trail when it moves.", composition::cursor_over_window),
        case("cursor-edge-shapes", "Hovering each resize edge and corner shows that edge's cursor on every target.", composition::cursor_edge_shapes),
        case("cursor-stationary-shape", "A cursor shape change with the pointer still is shown.", composition::cursor_stationary_shape),
        case("clock-test-config", "In the test configuration the taskbar clock shows the UI clock's wall time.", composition::clock_test_config),
        case("no-fps-overlay", "The test configuration draws no frame-rate text.", composition::no_fps_overlay),
    ]),
    category("stacking", "Stacking & minimize", &[
        case("click-raises", "Clicking an exposed lower client raises and composes that stable window id", stacking::click_raises),
        case("minimize-hides", "Minimizing removes client content and exposes the exact lower window", stacking::minimize_hides),
        case("restore", "Appbar restore repaints the client and restores keyboard eligibility", stacking::restore),
        case("all-minimized", "All minimized windows have no published focus and receive no typed input", stacking::all_minimized),
        case("exit-restacks", "Surviving clients keep their order when another owner exits", stacking::exit_restacks),
        case("raise-stable-id", "Raising by window id raises that window even after others exit.", stacking::raise_stable_id),
        case("appbar-order", "Appbar order follows creation, not stacking.", stacking::appbar_order),
        case("minimize-no-input", "Keys and clicks never reach a minimized window and are recorded as rejected.", stacking::minimize_no_input),
        case("minimize-close", "Closing a minimized window removes its appbar entry.", stacking::minimize_close),
        case("fkeys-skip-minimized", "Function-key selection skips minimized windows.", stacking::fkeys_skip_minimized),
        case("chromeless-top", "A chromeless window stays above normal windows.", stacking::chromeless_top),
        case("dimmer-only-with-chromeless", "The dimmer appears only while a chromeless window is shown.", stacking::dimmer_only_with_chromeless),
        case("launcher-toggle", "The launcher hotkey shows and hides a chromeless test client.", stacking::launcher_toggle),
        case("close-focus-next", "Closing the focused window focuses the next one in the stack.", stacking::close_focus_next),
        case("new-on-top", "A new window opens on top and focused.", stacking::new_on_top),
        case("registry-agrees", "After each of 16 operations, the kernel's z order equals the model's.", stacking::registry_agrees),
        case("model-sequence", "50 seeded random raise, minimize, restore and close steps match the operation model at every step.", stacking::model_sequence),
        case("minimize-all-restore-all", "Minimizing and restoring every window returns the original stack and screen.", stacking::minimize_all_restore_all),
        case("titlebar-drag-raises", "Starting a titlebar drag raises the window.", stacking::titlebar_drag_raises),
    ]),
    category("focus", "Focus, keyboard routing & hotkeys", &[
        case("initial-focus", "The initial client receives one FocusGained and is published focused", focus::initial_focus),
        case("pair-click", "A click transition sends one correct lost and gained pair", focus::focus_pair),
        case("keys-to-focused", "A typed A reaches the focused client and no other client", focus::keys_focused),
        case("text-exact", "Typing Hello, World! delivers exactly those thirteen UTF-8 bytes", focus::text_exact),
        case("releases", "A physical A release reaches the client that received the press", focus::releases),
        case("ctrl-c", "Ctrl+C arrives as Linux C with Ctrl distinct from Super", focus::ctrl_c),
        case("navigation-keys", "Home End Delete Page Up Page Down and arrows retain their key identities", focus::navigation),
        case("escape", "A standalone Escape reaches the focused client", focus::escape),
        case("cycle-next", "Alt+Tab visits both clients and wraps with correct focus notices", focus::cycle_next),
        case("pair-appbar", "An appbar focus change sends exactly one correct pair.", focus::pair_appbar),
        case("pair-cycle", "A hotkey focus change sends exactly one correct pair.", focus::pair_cycle),
        case("focus-after-close", "Closing the focused window sends FocusGained to its successor.", focus::focus_after_close),
        case("cycle-prev", "Alt+Shift+Tab visits them in reverse.", focus::cycle_prev),
        case("cycle-skips-minimized", "Cycling skips minimized windows.", focus::cycle_skips_minimized),
        case("release-follows-press", "A key release goes to the window that got the press, even after focus moved.", focus::release_follows_press),
        case("focus-change-mid-press", "A focus change between press and release sends no release to the new window.", focus::focus_change_mid_press),
        case("repeat-delay", "A key held to just before the repeat delay gives no repeat, and at the delay gives one.", focus::repeat_delay),
        case("repeat-rate", "A key held for 1,000 ms on the UI clock gives exactly 11 repeats (500 ms delay, then every 50 ms, inclusive).", focus::repeat_rate),
        case("repeat-cancel", "Releasing the key or removing its device stops repeats at once.", focus::repeat_cancel),
        case("repeat-frozen-clock", "With the UI clock frozen, a held key gives no repeats, and the case still ends by its real deadline.", focus::repeat_frozen_clock),
        case("super-return", "Super+Return runs its configured command.", focus::super_return),
        case("alt-q", "Alt+Q sends CloseRequested to the focused window.", focus::alt_q),
        case("hotkey-consumed", "A hotkey is recorded as WM-handled and not delivered, and the same key without the modifier is delivered.", focus::hotkey_consumed),
        case("super-double-tap", "Two Super taps exactly at the interval toggle the launcher, and one tick over the interval does not.", focus::super_double_tap),
        case("super-single-tap", "A single Super tap does nothing.", focus::super_single_tap),
        case("hotkeys-config", "A binding in hotkeys.conf works on every target.", focus::hotkeys_config),
        case("no-focus-no-delivery", "With no window, typed keys are recorded as rejected for no focus, after the same keys reached a focused window.", focus::no_focus_no_delivery),
        case("focus-new-window", "A newly opened window takes focus.", focus::focus_new_window),
        case("routing-after-exit", "After another window's client exits, keys still reach the focused window.", focus::routing_after_exit),
        case("lost-then-delivered", "A key lost to a full ring followed by a delivered key leaves the lost serial recorded as lost.", focus::lost_then_delivered),
    ]),
    category("pointer", "Pointer routing", &[
        case("local-coords", "Pointer motion reaches the client at exact content-local coordinates", pointer::local_coordinates),
        case("capture", "A pressed client receives motion and release outside its bounds", pointer::capture),
        case("right-middle", "Right and middle presses and releases reach their target", pointer::right_middle),
        case("scroll-under-pointer", "Wheel input wakes and reaches the pointer target rather than the focused client", pointer::scroll),
        case("outside-not-delivered", "After positive delivery outside motion reaches no uncaptured client", pointer::outside_none),
        case("press-release-order", "Press motion and release reach the client in serial order", pointer::press_order),
        case("capture-target-closes", "When the captured window is destroyed, the capture ends and no other window gets the release.", pointer::capture_target_closes),
        case("scroll-wakes", "Scroll on an idle desktop wakes the compositor and is delivered.", pointer::scroll_wakes),
        case("fast-click", "A press and release within one composite are both delivered.", pointer::fast_click),
        case("double-click-threshold", "Two clicks exactly at the interval arrive with click counts 1 and 2, and one tick over gives 1 and 1.", pointer::double_click_threshold),
        case("double-click-distance", "Two clicks 4 pixels apart give click counts 1 and 2, and 5 pixels apart on either axis give 1 and 1.", pointer::double_click_distance),
        case("chrome-click", "Clicks on chrome are recorded as WM-handled and not delivered.", pointer::chrome_click),
        case("enter-leave", "Crossing from one window to another delivers leave and enter.", pointer::enter_leave),
        case("cursor-position", "The cursor is drawn at the injected position.", pointer::cursor_position),
        case("modifier-click", "Shift+click arrives with the Shift modifier.", pointer::modifier_click),
        case("clamp", "Coordinates past the screen edge clamp to it.", pointer::clamp),
        case("minimized-none", "A minimized window receives no pointer events.", pointer::minimized_none),
        case("chromeless-click", "Clicks on a chromeless window reach it, and clicks outside it dismiss it.", pointer::chromeless_click),
        case("press-raises-delivers", "A press on a lower window raises it and is delivered once.", pointer::press_raises_delivers),
        case("hit-test", "bwm's hit test returns the topmost window at a point inside a partly covered window.", pointer::hit_test),
        case("stale-scene", "A pointer event tagged with an older scene version is recorded as rejected for a stale scene when it starts a gesture, while a later batch of a gesture already under bwm's grab is delivered to the grabbing window even though the scene changed (section 5.5).", pointer::stale_scene),
    ]),
    category("geometry", "Move & resize", &[
        case("drag-move", "A titlebar drag moves the client by the exact drag delta and repaints it", geometry::drag_move),
        case("resize-right", "Right-edge drag requests owner resize changes only width and composes it", geometry::resize_right),
        case("resize-bottom", "Bottom-edge drag requests owner resize changes only height and composes it", geometry::resize_bottom),
        case("client-refuses", "An owner whose resize allocation exceeds RLIMIT_AS reports ResizeFailed and retains its original frame", geometry::resize_refused),
        case("resize-cross", "Crossing the opposite edge clamps to the minimum and never requests a huge width", geometry::resize_cross),
        case("client-resize", "bwm reconciles and composes a client-initiated resize", geometry::client_resize),
        case("drag-top-clamp", "A window cannot be dragged above the taskbar.", geometry::drag_top_clamp),
        case("drag-keeps-titlebar", "A window dragged off a side keeps the contract's minimum titlebar strip on screen.", geometry::drag_keeps_titlebar),
        case("resize-left", "Dragging the left edge moves the left side and keeps the right side fixed.", geometry::resize_left),
        case("resize-top", "Dragging the top edge moves the top side and keeps the bottom side fixed.", geometry::resize_top),
        case("resize-corners", "Each corner resizes both dimensions, anchored at the opposite corner.", geometry::resize_corners),
        case("resize-minimum", "A resize stops at the minimum width and height.", geometry::resize_minimum),
        case("resize-maximum", "A resize stops at the screen and buffer limits.", geometry::resize_maximum),
        case("configure-ack", "The frame changes only after LIST reports the owner's new size.", geometry::configure_ack),
        case("buffer-matches", "After a resize, the client's buffer size equals the frame's content size.", geometry::buffer_matches),
        case("coalesce", "At most one ResizeRequested remains outstanding until LIST reports a size change.", geometry::coalesce),
        case("exit-during-drag", "A client exiting mid-drag ends the drag, and no other window moves.", geometry::exit_during_drag),
        case("exit-during-resize", "A client exiting mid-resize ends the resize, and no other window changes.", geometry::exit_during_resize),
        case("client-title", "A client's title change shows in its titlebar and appbar.", geometry::client_title),
        case("remembered-geometry", "Reopening a window with the same title restores its last position and size.", geometry::remembered_geometry),
        case("expose-during-drag", "Windows beneath repaint exactly at each step of a drag.", geometry::expose_during_drag),
        case("resize-repaints", "During a resize, the uncovered desktop and windows repaint exactly.", geometry::resize_repaints),
        case("geometry-registry", "After every move and resize, the registry's geometry equals the model's.", geometry::geometry_registry),
        case("chromeless-not-resizable", "A chromeless window has no resize edges.", geometry::chromeless_not_resizable),
    ]),
    category("lifecycle", "Close, crash & launch", &[
        case("close-request", "The first close delivers CloseRequested without killing an ignoring owner", lifecycle::close_request),
        case("client-exit", "Normal client exit removes its window and repaints the survivor", lifecycle::client_exit),
        case("client-crash", "SIGSEGV removes the crashed window and repaints the survivor", lifecycle::client_crash),
        case("client-exec", "Exec removes the old window and repaints the survivor", lifecycle::client_exec),
        case("close-unresponsive", "A second close sends SIGTERM to an owner that ignored the first request", lifecycle::close_unresponsive),
        case("compositor-steal", "Another process cannot steal the live bwm compositor claim", lifecycle::compositor_steal),
        case("inject-denied", "A non-compositor cannot inject events while bwm owns the claim", lifecycle::inject_denied),
        case("launch-ack", "Super+Return launches a real GUI client whose first commit reaches device scanout", lifecycle::launch_ack),
        case("launch-reaped", "bwm reaps the GUI client its hotkey launched after that client exits", lifecycle::launch_reaped),
        case("close-one-of-two", "Closing one of a process's two windows leaves the other open.", lifecycle::close_one_of_two),
        case("launch-missing", "A hotkey bound to a missing program shows an error on the desktop and leaves no zombie.", lifecycle::launch_missing),
        case("launch-fork-fails", "A launch whose fork fails by test control shows an error, and the next launch succeeds.", lifecycle::launch_fork_fails),
        case("launch-close-50", "50 launch and close cycles return windows, processes and memory to the baseline.", lifecycle::launch_close_50),
        case("read-other-denied", "A non-root client cannot capture another window.", lifecycle::read_other_denied),
        case("huge-window", "A client's 4096x4096 window is clamped on screen, and the desktop stays usable.", lifecycle::huge_window),
        case("commit-flood", "A client committing as fast as it can does not delay input to other windows.", lifecycle::commit_flood),
        case("title-flood", "A client changing its title 10,000 times does not stall the desktop.", lifecycle::title_flood),
        case("never-reads", "A client that never reads events loses only its own events, each recorded as lost.", lifecycle::never_reads),
        case("normal-exit-gives-back", "bwm's normal exit gives the display back.", lifecycle::normal_exit_gives_back),
        case("window-limit", "A seventeenth window is refused visibly, and the others are unaffected.", lifecycle::window_limit),
    ]),
    category("recovery", "WM death, restart & supervision", &[
        case("clients-notified", "Killing the real bwm delivers CompositorChanged with its epoch", recovery::clients_notified),
        case("restart-adopts", "A restarted real bwm composes the surviving client and its retained pixels", recovery::restart_adopts),
        case("interaction-after-restart", "The same surviving client receives keys and clicks under the next bwm", recovery::interaction_restart),
        case("stack-restored", "Restart preserves published geometry stacking and minimized state", recovery::stack_restored),
        case("supervisor-restarts", "Production init desktop restarts a killed bwm and recomposes survivors", recovery::supervisor_restarts),
        case("death-releases", "When bwm is killed, the display returns to the session supervisor, grabs and pressed state reset, and the epoch advances.", recovery::death_releases),
        case("blocked-acquire", "A client blocked in acquire when bwm dies returns within 250 ms.", recovery::blocked_acquire),
        case("blocked-wait", "A client blocked in wait_event when bwm dies returns within 250 ms.", recovery::blocked_wait),
        case("death-at-composite", "bwm killed while composing a client's commit leaves that client able to commit to the next bwm.", recovery::death_at_composite),
        case("death-mid-configure", "A resize request pending at compositor death leaves the owner's size unchanged and the next bwm reconciles it.", recovery::death_mid_configure),
        case("death-mid-capture", "A pointer capture held at bwm's death ends, and the next bwm starts with no capture.", recovery::death_mid_capture),
        case("focus-restored", "The restarted bwm restores focus to the window that had it.", recovery::focus_restored),
        case("clipboard-reset", "A restarted bwm starts with an empty clipboard, as the contract says.", recovery::clipboard_reset),
        case("supervisor-gives-up", "After five restarts within ten seconds the supervisor stops and reports the crash loop.", recovery::supervisor_gives_up),
        case("unexpected-crash-fails", "An app crash during a deliberate bwm restart fails the scenario, distinct from the expected compositor death.", recovery::unexpected_crash_fails),
    ]),
    category("clipboard", "Clipboard between programs", &[
        case("cross-process", "Two real window-owning clients transfer exact text and change serial through bwm", clipboard::clip_cross),
        case("utf8", "Cross-process clipboard transfer preserves multi-byte UTF-8 byte for byte", clipboard::clip_utf8),
        case("large", "One MiB transfers between independent clipboard peers", clipboard::clip_large),
        case("nul-bytes", "Clipboard transfer preserves embedded NUL bytes", clipboard::clip_nul),
        case("replace", "A second peer replaces clipboard text with a later change serial", clipboard::clip_replace),
        case("owner-exit", "Clipboard text and serial survive the setting client exit", clipboard::clip_owner_exit),
        case("set-get", "A program reads back the text it put on the clipboard.", clipboard::set_get),
        case("empty", "Getting an empty clipboard returns empty, not an error.", clipboard::empty),
        case("too-large", "Text over the 16 MiB limit is refused with E2BIG, and the clipboard keeps its previous text.", clipboard::too_large),
        case("change-notice", "Programs that asked are told when the clipboard changes.", clipboard::change_notice),
        case("concurrent-sets", "Of two concurrent sets, the one with the later clipboard serial wins, and both setters see that result.", clipboard::concurrent_sets),
        case("mime-text-only", "A type other than text/plain;charset=utf-8 is refused.", clipboard::mime_text_only),
        case("window-required", "A process with no window on the desktop is refused.", clipboard::window_required),
        case("unprivileged", "A uid 1000 client with a window can set and get.", clipboard::unprivileged),
        case("textfield-copy", "Ctrl+C in a libbui text field copies its selection.", clipboard::textfield_copy),
        case("textfield-paste", "Ctrl+V in a libbui text field inserts the clipboard text.", clipboard::textfield_paste),
    ]),
    category("liveness", "Idle, stalls & stress", &[
        case("compositor-wait-timeout", "The v2 scene poll times out within +100 ms when idle and wakes before timeout after a client commit", liveness::compositor_wait_timeout),
        case("idle-no-composites", "One hundred acknowledged idle UI epochs produce no composites", liveness::idle_composites),
        case("hung-client", "A stopped client cannot prevent another client receiving input and repainting", liveness::hung_client),
        case("wake-on-registry", "An independent new client wakes an idle bwm and reaches scanout", liveness::wake_registry),
        case("stress-animation", "Eight clients advance through 300 UI epochs while input is processed", liveness::stress_clients),
        case("memory-bounded", "After 60 warmup epochs 600 epochs of eight clients grow bwm RSS by at most 64 KiB", liveness::memory_bounded),
        case("deadline-wake", "bwm wakes for its next clock deadline with no other work.", liveness::deadline_wake),
        case("slow-client", "A client committing every tenth epoch does not reduce other clients' frames per epoch.", liveness::slow_client),
        case("stopped-client", "A client stopped with SIGSTOP and explicitly excluded does not stop the desktop, and stays stale.", liveness::stopped_client),
        case("stress-io", "Animation plus disk and loopback network traffic still processes input in order, with every disposition recorded.", liveness::stress_io),
        case("progress-under-load", "Five seconds of multi-window composition raise the composite counter in every 100 ms window, and the soft-lockup counter stays unchanged.", liveness::progress_under_load),
        case("cursor-shape-idle", "A cursor shape change on an idle desktop is shown.", liveness::cursor_shape_idle),
    ]),
    category("performance", "Latency & frame rate", &[
        case("typing-latency", "One hundred measured keys reach composited handling commits within median 20 ms and max 50 ms", performance::typing_latency),
        case("pointer-latency", "One hundred measured motions reach composited handling commits within median 20 ms and max 50 ms", performance::pointer_latency),
        case("present-latency", "Three hundred measured 256x256 commits reach scanout within median 16 ms and max 33 ms", performance::present_latency),
        case("frame-floor", "A client animating as fast as released receives at least 60 composites per second over five seconds", performance::frame_floor),
        case("idle-cpu", "bwm and three static clients use at most two percent of one CPU over five seconds", performance::idle_cpu),
        case("latency-under-animation", "Four animating clients retain median 33 ms and max 100 ms input latency", performance::animation_latency),
        case("clipboard-latency", "Eighteen measured 64 KiB transfers between programs each finish within 50 ms", performance::clipboard_latency),
    ]),
]).gui();

#[path = "suite_breengel/stage_painter.rs"]
mod stage_painter;

fn main() {
    breenix_suite::suite::set_painter(Box::new(stage_painter::StagePainter::new()));
    SUITE.run()
}
