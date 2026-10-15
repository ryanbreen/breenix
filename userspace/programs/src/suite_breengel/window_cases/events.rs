//! events: window input events (contract section 19).

use crate::*;

pub(crate) const EVENTS: &[Case] = &[
    case("key-press-release", "A key press and its release arrive with the same keycode", events_key_press_release),
    case("key-text", "A key event carries the UTF-8 text the compositor attached", events_key_text),
    case("key-modifiers", "Shift, Ctrl, Alt and Super arrive as separate modifier bits", events_key_modifiers),
    case("key-repeat", "A repeat arrives marked as a repeat", events_key_repeat),
    case("text-event", "A text event delivers its UTF-8 string whole", events_text_event),
    case("mouse-move", "Pointer motion arrives in window-local coordinates", events_mouse_move),
    case("mouse-buttons", "Left, right and middle press and release arrive with the right button", events_mouse_buttons),
    case("scroll", "Vertical and horizontal scroll arrive with their signed amounts", events_scroll),
    case("focus", "FocusGained and FocusLost arrive as sent", events_focus),
    case("close-requested", "CloseRequested arrives as sent", events_close_requested),
    case("resized", "Resized arrives only after the buffers have the new size, and says that size", events_resized),
    case("resize-failed", "A failed resize delivers ResizeFailed with the old size, never Resized", events_resize_failed),
    case("font-changed", "FontChanged arrives when the configured font changes", events_font_changed),
    case("compositor-changed", "A new compositor claim delivers CompositorChanged with the new epoch", events_compositor_changed),
    case("unknown-type", "An unknown event type arrives as Unknown, not as a key press", events_unknown_type),
    case("order", "100 events arrive in the order sent", events_order),
    case("serial", "Each event carries its input serial", events_serial),
    case("processed-serial", "After the app's handlers return and it acquires, draws and commits, the commit's processed serial is the highest serial whose handler had returned at acquire", events_processed_serial),
    case("processed-not-polled", "An event polled but not yet handled when an animation frame is committed is not acknowledged by that commit", events_processed_not_polled),
    case("processed-at-acquire", "An event whose handler returns after frame N was acquired is acknowledged by frame N + 1, not by frame N", events_processed_at_acquire),
    case("no-change-after-redraw", "commit_no_change does not acknowledge an event whose handler returned Redraw until a frame acquired after it is committed", events_no_change_after_redraw),
    case("processed-ack", "commit_no_change publishes the processed serial without changing pixels", events_processed_ack),
    case("overflow", "A full ring reports how many events were lost instead of dropping them silently", events_overflow),
    case("wait-timeout", "wait_event with a timeout returns no earlier than the timeout and within 100 ms after it when nothing arrives", events_wait_timeout),
    case("wait-eintr", "A signal interrupts a blocked wait_event with EINTR", events_wait_eintr),
    case("wait-targeted", "An event for one window does not wake a reader blocked on another", events_wait_targeted),
    case("pollable", "A window's event descriptor reports POLLIN through poll when an event is queued", events_pollable),
    case("per-window", "Two windows of one process each receive only their own events", events_per_window),
    case("after-destroy", "Writing to a destroyed window fails with ENOENT", events_after_destroy),
    case("abi-layout", "The event struct has the contract's literal size and field offsets", events_abi_layout),
    case("no-overloaded-fields", "Resize size, button state and text travel in named fields, not borrowed ones", events_no_overloaded_fields),
    case("malformed-write", "A compositor write with an invalid type or length fails with EINVAL and delivers nothing", events_malformed_write),
];

// ---------------------------------------------------------------------------------------
// events: window input events (contract section 19)
// ---------------------------------------------------------------------------------------

fn events_key_press_release() -> CaseResult { needs_window("19.1", "KeyPress and KeyRelease with the same keycode") }
fn events_key_text() -> CaseResult { needs_window("19.1", "a KeyPress carrying the compositor's UTF-8 text") }
fn events_key_modifiers() -> CaseResult { needs_window("19.1", "Shift, Ctrl, Alt and Super as separate modifier bits") }
fn events_key_repeat() -> CaseResult { needs_window("19.1", "a repeat marked in flags") }
fn events_text_event() -> CaseResult { needs_window("19.1", "a Text event's whole UTF-8 string") }
fn events_mouse_move() -> CaseResult { needs_window("19.1", "PointerMove in window-local coordinates") }
fn events_mouse_buttons() -> CaseResult { needs_window("19.1", "left, right and middle PointerButton events") }
fn events_scroll() -> CaseResult { needs_window("19.1", "signed vertical and horizontal Scroll") }
fn events_focus() -> CaseResult { needs_window("19.1", "FocusGained and FocusLost") }
fn events_close_requested() -> CaseResult { needs_window("19.1", "CloseRequested") }
fn events_resized() -> CaseResult { needs_window("19.2", "Resized after the buffers have the new size") }
fn events_resize_failed() -> CaseResult { needs_window("19.2", "ResizeFailed with the old size, never Resized") }
fn events_font_changed() -> CaseResult { needs_window("9.3, 19.1", "FontChanged from Breengel's font watcher") }
fn events_compositor_changed() -> CaseResult { needs_window("18, 19.1", "CompositorChanged with the new epoch") }
fn events_unknown_type() -> CaseResult { needs_window("19.1", "an unknown kind delivered as Event::Unknown") }
fn events_order() -> CaseResult { needs_window("19.2", "100 events in the order written") }
fn events_serial() -> CaseResult { needs_window("19.3", "each event's input serial") }
fn events_processed_serial() -> CaseResult { needs_window("19.4", "the processed watermark recorded at ACQUIRE") }
fn events_processed_not_polled() -> CaseResult { needs_window("19.4", "an unhandled event not acknowledged by an animation frame") }
fn events_processed_at_acquire() -> CaseResult { needs_window("19.4", "an event handled after frame N's acquire acknowledged by frame N + 1") }
fn events_no_change_after_redraw() -> CaseResult { needs_window("19.4", "COMMIT_NO_CHANGE withholding a Redraw event's acknowledgment") }
fn events_processed_ack() -> CaseResult { needs_window("17.3, 19.4", "COMMIT_NO_CHANGE publishing the processed serial") }
fn events_overflow() -> CaseResult { needs_window("19.2", "an Overflow event counting lost events") }
fn events_wait_timeout() -> CaseResult { needs_window("17.5", "READ_EVENTS's timeout plus at most 100 ms") }
fn events_wait_eintr() -> CaseResult { needs_window("17.1", "EINTR for a blocked READ_EVENTS") }
fn events_wait_targeted() -> CaseResult { needs_window("19.2", "an event waking only its own window's reader") }
fn events_pollable() -> CaseResult { needs_window("19.2", "POLLIN on EVENT_FD") }
fn events_per_window() -> CaseResult { needs_window("19.2", "two windows of one process receiving only their own events") }
fn events_after_destroy() -> CaseResult { needs_window("19.2", "ENOENT for WRITE_EVENTS to a destroyed window") }
fn events_abi_layout() -> CaseResult { needs_window("19.1", "the 320-byte WindowEvent layout, read raw") }
fn events_no_overloaded_fields() -> CaseResult { needs_window("19.1", "resize size, button state and text in their own fields") }
fn events_malformed_write() -> CaseResult { needs_window("19.2", "EINVAL and nothing delivered for a bad kind or text length") }
