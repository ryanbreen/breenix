//! #482 review fix pass -- `scripts/parallels/launcher-smoke.sh` must
//! preserve the guest's raw early-boot `[xhci]` descriptor-enumeration trail
//! (port scan, EnableSlot, get-descriptor/set-configuration lines)
//! alongside the one-line `start_hid_polling` summary it already captures
//! into `hid-poll-line.txt`.
//!
//! Why: a 2026-09-06/07 round doc
//! (docs/planning/green-program/input-usb/482-XHCI-ARM-BEFORE-KICK-2026-09-06.md)
//! implied every fixed-branch launcher-smoke/type-filter run preserved real
//! device/configuration-descriptor enumeration evidence. In fact two of the
//! four runs cited (`launcher-smoke/run2`, `type-filter/run2-pass`)
//! preserved zero `[xhci]` lines anywhere in their evidence directory --
//! only the single `hid-poll-line.txt` summary line, grepped from the
//! authoritative `$SERIAL_LOG`. The fix this suite pins: `launcher-smoke.sh`
//! now greps `$SERIAL_LOG` (the same authoritative source `HID_POLL_LINE`
//! already reads, not the trigger-onward `serial-excerpt.txt`) for every
//! `[xhci]` line into a dedicated `$EVIDENCE_DIR/xhci-enum-excerpt.txt`,
//! written unconditionally (pass or fail), the same way `hid-poll-line.txt`
//! already is.
//!
//! This is a text-shape check over the script's own source -- the same
//! technique `tests/parallels_kill_by_name_structure.rs` uses for a
//! different Parallels-script hazard: no shell execution, no VM, host-side
//! only.

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn launcher_smoke_sh() -> String {
    fs::read_to_string(repo_root().join("scripts/parallels/launcher-smoke.sh"))
        .expect("read scripts/parallels/launcher-smoke.sh")
}

/// `true` if `source` contains a line, outside a `#` comment, that greps
/// `[xhci]` lines out of `$SERIAL_LOG` into a file under `$EVIDENCE_DIR` --
/// the shape this suite requires, not one specific byte-for-byte command (a
/// reasonable rewording of the same shell command still passes).
fn captures_xhci_enum_excerpt(source: &str) -> bool {
    source.lines().any(|line| {
        let trimmed = line.trim_start();
        !trimmed.starts_with('#')
            && trimmed.contains("grep")
            && trimmed.contains(r"\[xhci\]")
            && trimmed.contains("\"$SERIAL_LOG\"")
            && trimmed.contains("$EVIDENCE_DIR")
    })
}

#[test]
fn launcher_smoke_captures_raw_xhci_enum_trail() {
    let source = launcher_smoke_sh();
    assert!(
        captures_xhci_enum_excerpt(&source),
        "scripts/parallels/launcher-smoke.sh must grep '[xhci]' lines out of \
         \"$SERIAL_LOG\" into a file under \"$EVIDENCE_DIR\" so every future run \
         preserves raw descriptor-enumeration evidence, not only the \
         start_hid_polling summary line (#482 review fix pass, X-2)"
    );
}

#[test]
fn deliberately_broken_copy_reddens_the_rule() {
    let source = launcher_smoke_sh();
    // Simulate the pre-fix script: drop the capture line the real predicate would
    // match.
    let mutated: String = source
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !(!trimmed.starts_with('#')
                && trimmed.contains("grep")
                && trimmed.contains(r"\[xhci\]")
                && trimmed.contains("\"$SERIAL_LOG\"")
                && trimmed.contains("$EVIDENCE_DIR"))
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !captures_xhci_enum_excerpt(&mutated),
        "the predicate must redden once the xhci-enum-excerpt capture line is removed"
    );
    // Control: the real, unmutated script passes.
    assert!(captures_xhci_enum_excerpt(&source));
}
