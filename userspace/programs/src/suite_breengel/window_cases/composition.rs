//! The window cases of composition (contract section 18).

use crate::*;

pub(crate) const COMPOSITION: &[Case] = &[
    case("compose-position", "A committed pattern appears at the window's position in the composed frame, exactly", composition_compose_position),
    case("compose-overlap", "Two overlapping windows compose in z order, exactly", composition_compose_overlap),
    case("compose-offscreen", "A window partly off the screen is clipped at the screen edge", composition_compose_offscreen),
    case("compose-fullscreen", "A 1280x800 window at 0,0 fills the frame exactly", composition_compose_fullscreen),
    case("compose-many", "Sixteen windows in a fixed stack compose to the formula, exactly", composition_compose_many),
    case("generation", "Each commit advances the window's commit id by exactly one", composition_generation),
    case("no-compositor", "A commit with no compositor returns within 250 ms and is composed once a compositor claims", composition_no_compositor),
    case("compositor-dies", "An acquire blocked when the compositor exits returns within 250 ms, and the next commit succeeds", composition_compositor_dies),
    case("eintr", "A signal interrupts a blocked acquire with EINTR", composition_eintr),
    case("damage-metadata", "A commit's damage rectangles are recorded exactly", composition_damage_metadata),
    case("damage-work", "A composite processes exactly the pixels in its damage union, by the compositor's counter, and with `ignore-damage` armed the excess is reported by count", composition_damage_work),
    case("damage-screen", "A partial composite gives the same frame as a full recomposition", composition_damage_screen),
    case("composite-record", "Each composite publishes its serial and the commit id and pixel generation of every presentation-required window it accounts for, a wholly occluded one included with zero composed pixels", composition_composite_record),
    case("backend-error", "A forced backend failure leaves the commit uncomposited, is reported to the compositor, and the retry succeeds", composition_backend_error),
    case("inflight-bound", "At most one composite per window is in flight", composition_inflight_bound),
    case("no-change-commit", "commit_no_change publishes new metadata with empty damage and the same pixel generation, causes no composite, and is presented by the composite that already holds that generation", composition_no_change_commit),
    case("cursor-hidden", "With the cursor hidden, no cursor pixel appears in the frame", composition_cursor_hidden),
    case("cursor-no-trail", "Moving the cursor over a static scene restores every pixel it covered", composition_cursor_no_trail),
    case("cursor-shape", "Setting a cursor shape draws that shape at the pointer", composition_cursor_shape),
    case("cursor-stationary-repaint", "Content committed under a stationary cursor is shown, with the cursor redrawn above it", composition_cursor_stationary_repaint),
    case("flush-irq-bound", "The longest IRQ-masked interval during full-screen damage stays within 500 µs, after `irq-hold` has shown the gauge reads a 2 ms hold", composition_flush_irq_bound),
    case("present-rate", "One 256x256 window composes at least 60 commits a second", composition_present_rate),
    case("takeover-giveback", "Display takeover and giveback restore the previous owner's frame exactly", composition_takeover_giveback),
    case("ownership-stack", "When a compositor dies, the display returns to the process that gave it, not to the runner", composition_ownership_stack),
];

pub(crate) const COMPOSITION_FLUSH: &[Case] = &[
    case("flush-clip", "FLUSH clips its rectangle to the screen", composition_flush_clip),
];

// ---------------------------------------------------------------------------------------
// composition: commit, composition & present (contract section 18)
// ---------------------------------------------------------------------------------------

fn composition_compose_position() -> CaseResult { needs_window("18", "a committed pattern composed at its window's position") }
fn composition_compose_overlap() -> CaseResult { needs_window("18", "overlapping windows composed in z order") }
fn composition_compose_offscreen() -> CaseResult { needs_window("18", "a window clipped at the screen's edge") }
fn composition_compose_fullscreen() -> CaseResult { needs_window("18", "a 1280x800 window at 0,0 filling the frame") }
fn composition_compose_many() -> CaseResult { needs_window("18", "sixteen stacked windows composed to the formula") }
fn composition_generation() -> CaseResult { needs_window("17.3", "commit ids advancing by exactly one") }
fn composition_no_compositor() -> CaseResult { needs_window("17.3, 18", "a commit with no compositor returning within 250 ms and composed after a claim") }
fn composition_compositor_dies() -> CaseResult { needs_window("17.5, 18", "a blocked ACQUIRE returning within 250 ms of the compositor's exit") }
fn composition_eintr() -> CaseResult { needs_window("17.3", "EINTR for a blocked ACQUIRE") }
fn composition_damage_metadata() -> CaseResult { needs_window("17.3", "a commit's damage rectangles recorded exactly") }
fn composition_damage_work() -> CaseResult { needs_window("18, 21", "composite pixel counts and the ignore-damage twin") }
fn composition_damage_screen() -> CaseResult { needs_window("18", "a partial composite equal to a full recomposition") }
fn composition_composite_record() -> CaseResult { needs_window("18", "COMPOSITE_RECORD listing every presentation-required window") }
fn composition_backend_error() -> CaseResult { needs_window("18, 21", "a forced backend failure reported, then retried") }
fn composition_inflight_bound() -> CaseResult { needs_window("18", "at most one composite per window in flight") }
fn composition_no_change_commit() -> CaseResult { needs_window("17.3, 18", "COMMIT_NO_CHANGE with empty damage, the same pixel generation and no composite") }
fn composition_cursor_hidden() -> CaseResult { needs_window("18", "no cursor pixel with the cursor hidden") }
fn composition_cursor_no_trail() -> CaseResult { needs_window("18", "a moving cursor restoring every pixel it covered") }
fn composition_cursor_shape() -> CaseResult { needs_window("18", "SET_CURSOR's shape drawn at the pointer") }
fn composition_cursor_stationary_repaint() -> CaseResult { needs_window("18", "content under a stationary cursor shown with the cursor above it") }
fn composition_flush_irq_bound() -> CaseResult { needs_window("18, 21", "the 500 us IRQ-masked bound after the irq-hold twin") }
fn composition_present_rate() -> CaseResult { needs_window("18", "at least 60 composited commits a second") }
fn composition_takeover_giveback() -> CaseResult { needs_window("18, 20", "the previous owner's frame restored exactly, by SCANOUT_READBACK") }
fn composition_ownership_stack() -> CaseResult { needs_window("18", "the display returning to the process that gave it") }

fn composition_flush_clip() -> CaseResult { needs_window("18, 20", "FLUSH clipping its rectangle, observed by SCANOUT_READBACK") }
