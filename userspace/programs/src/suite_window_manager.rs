//! E18: real bwm, independent GPU clients and device-backed scanout observation.
#[path = "wm_suite/cases.rs"]
mod cases;
#[path = "wm_suite/desktop.rs"]
mod desktop;
#[path = "wm_suite/input.rs"]
mod input;
#[path = "wm_suite/measure.rs"]
mod measure;
#[path = "wm_suite/protocol.rs"]
mod protocol;
use breenix_suite::suite::{case, category, suite, Suite};

static SUITE: Suite = suite("window-manager", "Window manager", &[
    category("input-core", "Input core & virtual devices", &[
        case("virtual-keyboard", "A virtual A press and release traverse the core and bwm to a real client", cases::virtual_keyboard),
        case("serials", "Input serials increase across two virtual devices and bwm delivers both", cases::core_serials),
        case("time-injected", "Injector timestamps survive routing and decreasing time is refused", cases::core_timestamps),
        case("ctrl-super-distinct", "Virtual Ctrl+A reaches a client with Ctrl and without Super", cases::core_modifiers),
        case("text-event", "Whole UTF-8 EV_TEXT reaches a client and invalid UTF-8 is refused", cases::core_text),
        case("device-removal", "Removing a held virtual keyboard sends the release to its client", cases::core_removal),
        case("virtual-pointer", "A virtual absolute pointer and button traverse the core and bwm at exact client coordinates", cases::virtual_pointer),
        case("ledger-complete", "A mixed virtual script has one terminal disposition per serial and delivered keys have processing commits", cases::core_ledger),
        case("capabilities", "Undeclared keys return EINVAL between two successful declared-key deliveries", cases::core_capabilities),
    ]),
    category("input-devices", "Keyboard & pointer drivers on every target", &[
        case("keyboard-completion-path", "The bound keyboard completion reports, wakes bwm, delivers A and rearms", cases::keyboard_completion),
        case("pointer-completion-path", "The bound pointer completion reports, wakes bwm, delivers motion and rearms", cases::pointer_completion),
        case("replay-report-fault", "Suppressing the production report loses the event after a positive delivery", cases::replay_faults),
    ]),
    category("startup", "bwm starts and shows clients", &[
        case("desktop-control", "A completed empty desktop exposes the literal taskbar pixel", cases::desktop_control),
        case("claims-display", "A live bwm retains the compositor claim after composing a client", cases::claims_display),
        case("first-client", "A real client pattern and commit appear in completed device scanout", cases::first_client),
        case("existing-windows", "bwm adopts and composes a window created before it starts", cases::existing_windows),
        case("independent-clients", "Two independent owners share the desktop and the top client is exact", cases::independent_clients),
        case("supervisor-starts", "Production init desktop starts bwm and composes an independent client", cases::supervisor_starts),
    ]),
    category("composition", "Composition, chrome & repaint", &[
        case("chrome-geometry", "Decorations and desktop bars occupy the literal rectangles and colours", cases::chrome),
        case("overlap", "The higher client covers the lower client with its exact complete pattern", cases::overlap),
        case("cascade", "New windows cascade by 50 logical pixels on each axis", cases::cascade),
        case("client-update", "A new client commit replaces its pattern in completed device scanout", cases::client_update),
        case("expose-close", "An exited top client reveals the exact complete lower client", cases::expose_close),
        case("oracle-corruption", "The scanout check rejects a wrong content pixel and missing commit provenance", cases::frame_integrity),
    ]),
    category("stacking", "Stacking & minimize", &[
        case("click-raises", "Clicking an exposed lower client raises and composes that stable window id", cases::click_raises),
        case("minimize-hides", "Minimizing removes client content and exposes the exact lower window", cases::minimize_hides),
        case("restore", "Appbar restore repaints the client and restores keyboard eligibility", cases::restore),
        case("all-minimized", "All minimized windows have no published focus and receive no typed input", cases::all_minimized),
        case("exit-restacks", "Surviving clients keep their order when another owner exits", cases::exit_restacks),
    ]),
    category("focus", "Focus, keyboard routing & hotkeys", &[
        case("initial-focus", "The initial client receives one FocusGained and is published focused", cases::initial_focus),
        case("pair-click", "A click transition sends one correct lost and gained pair", cases::focus_pair),
        case("keys-to-focused", "A typed A reaches the focused client and no other client", cases::keys_focused),
        case("text-exact", "Typing Hello World delivers exactly those eleven bytes", cases::text_exact),
        case("releases", "A physical A release reaches the client that received the press", cases::releases),
        case("ctrl-c", "Ctrl+C arrives as Linux C with Ctrl distinct from Super", cases::ctrl_c),
        case("navigation-keys", "Home End Delete Page Up Page Down and arrows retain their key identities", cases::navigation),
        case("escape", "A standalone Escape reaches the focused client", cases::escape),
        case("cycle-next", "Alt+Tab visits both clients and wraps with correct focus notices", cases::cycle_next),
    ]),
    category("pointer", "Pointer routing", &[
        case("local-coords", "Pointer motion reaches the client at exact content-local coordinates", cases::local_coordinates),
        case("capture", "A pressed client receives motion and release outside its bounds", cases::capture),
        case("right-middle", "Right and middle presses and releases reach their target", cases::right_middle),
        case("scroll-under-pointer", "Wheel input wakes and reaches the pointer target rather than the focused client", cases::scroll),
        case("outside-not-delivered", "After positive delivery outside motion reaches no uncaptured client", cases::outside_none),
        case("press-release-order", "Press motion and release reach the client in serial order", cases::press_order),
    ]),
    category("geometry", "Move & resize", &[
        case("drag-move", "A titlebar drag moves the client by the exact drag delta and repaints it", cases::drag_move),
        case("resize-right", "Right-edge drag requests owner resize changes only width and composes it", cases::resize_right),
        case("resize-bottom", "Bottom-edge drag requests owner resize changes only height and composes it", cases::resize_bottom),
        case("client-refuses", "An owner ignoring ResizeRequested keeps its whole original content and size", cases::resize_refused),
        case("resize-cross", "Crossing the opposite edge clamps to the minimum and never requests a huge width", cases::resize_cross),
        case("client-resize", "bwm reconciles and composes a client-initiated resize", cases::client_resize),
    ]),
    category("lifecycle", "Close, crash & launch", &[
        case("close-request", "The first close delivers CloseRequested without killing an ignoring owner", cases::close_request),
        case("client-exit", "Normal client exit removes its window and repaints the survivor", cases::client_exit),
        case("client-crash", "SIGSEGV removes the crashed window and repaints the survivor", cases::client_crash),
        case("client-exec", "Exec removes the old window and repaints the survivor", cases::client_exec),
        case("close-unresponsive", "A second close sends SIGTERM to an owner that ignored the first request", cases::close_unresponsive),
        case("compositor-steal", "Another process cannot steal the live bwm compositor claim", cases::compositor_steal),
        case("inject-denied", "A non-compositor cannot inject events while bwm owns the claim", cases::inject_denied),
        case("launch-ack", "Super+Return launches a real GUI client whose first commit reaches device scanout", cases::launch_ack),
        case("launch-reaped", "bwm reaps the GUI client its hotkey launched after that client exits", cases::launch_reaped),
    ]),
    category("recovery", "WM death, restart & supervision", &[
        case("clients-notified", "Killing the real bwm delivers CompositorChanged with its epoch", cases::clients_notified),
        case("restart-adopts", "A restarted real bwm composes the surviving client and its retained pixels", cases::restart_adopts),
        case("interaction-after-restart", "The same surviving client receives keys and clicks under the next bwm", cases::interaction_restart),
        case("stack-restored", "Restart preserves published geometry stacking and minimized state", cases::stack_restored),
        case("supervisor-restarts", "Production init desktop restarts a killed bwm and recomposes survivors", cases::supervisor_restarts),
    ]),
    category("clipboard", "Clipboard between programs", &[
        case("cross-process", "Two real window-owning clients transfer exact text and change serial through bwm", cases::clip_cross),
        case("utf8", "Cross-process clipboard transfer preserves multi-byte UTF-8 byte for byte", cases::clip_utf8),
        case("large", "One MiB transfers between independent clipboard peers", cases::clip_large),
        case("nul-bytes", "Clipboard transfer preserves embedded NUL bytes", cases::clip_nul),
        case("replace", "A second peer replaces clipboard text with a later change serial", cases::clip_replace),
        case("owner-exit", "Clipboard text and serial survive the setting client exit", cases::clip_owner_exit),
    ]),
    category("liveness", "Idle, stalls & stress", &[
        case("idle-no-composites", "One hundred acknowledged idle UI epochs produce no composites", cases::idle_composites),
        case("hung-client", "A stopped client cannot prevent another client receiving input and repainting", cases::hung_client),
        case("wake-on-registry", "An independent new client wakes an idle bwm and reaches scanout", cases::wake_registry),
        case("stress-animation", "Eight clients advance through 300 UI epochs while input is processed", cases::stress_clients),
        case("memory-bounded", "After 60 warmup epochs 600 epochs of eight clients grow bwm RSS by at most 64 KiB", cases::memory_bounded),
    ]),
    category("performance", "Latency & frame rate", &[
        case("typing-latency", "One hundred measured keys reach composited handling commits within median 20 ms and max 50 ms", cases::typing_latency),
        case("pointer-latency", "One hundred measured motions reach composited handling commits within median 20 ms and max 50 ms", cases::pointer_latency),
        case("present-latency", "Three hundred measured 256x256 commits reach scanout within median 16 ms and max 33 ms", cases::present_latency),
        case("frame-floor", "A client animating as fast as released receives at least 60 composites per second over five seconds", cases::frame_floor),
        case("idle-cpu", "bwm and three static clients use at most two percent of one CPU over five seconds", cases::idle_cpu),
        case("latency-under-animation", "Four animating clients retain median 33 ms and max 100 ms input latency", cases::animation_latency),
        case("clipboard-latency", "Eighteen measured 64 KiB transfers between programs each finish within 50 ms", cases::clipboard_latency),
    ]),
// No runner painting can overwrite a measured desktop; the scored GPU panel is drawn at the end.
]).defer_display();

fn main() {
    SUITE.run()
}
