//! scanout: display observation on every target (contract section 20).

use crate::*;
use crate::show::fbinfo;

pub(crate) const SCANOUT: &[Case] = &[
    case("mode-set", "A GUI suite boot runs this target's display at 1280x800, and fbinfo agrees", scanout_mode_set),
    case("device-path", "Readback uses this backend's observation path from the contract and names it", scanout_device_path),
    case("changing-scene", "Three successive known scenes each read back equal to their formulas", scanout_changing_scene),
    case("full-destination", "Readback covers the whole destination, and a change in its last row is seen", scanout_full_destination),
    case("binding", "The record names the scanout resource or address the device is bound to, and it is the composed one", scanout_binding),
    case("completion", "The record carries answered completion for every damaged rectangle of its composite serial", scanout_completion),
    case("atomic-record", "The pixels and the record always belong to the same composite serial under continuous commits", scanout_atomic_record),
    case("window-commits", "The record lists the commit id of each window composed", scanout_window_commits),
    case("format-canonical", "RGB and BGR destinations read back as the same canonical BGRX pixels", scanout_format_canonical),
    case("root-only", "A non-root, non-compositor process cannot read back the scanout", scanout_root_only),
    case("fault-skipped-transfer", "A skipped transfer or update is reported as missing completion, and the next composite recovers", scanout_fault_skipped_transfer),
    case("fault-wrong-resource", "A flush or scanout bound to the wrong resource is reported as a binding mismatch", scanout_fault_wrong_resource),
    case("fault-stale-fence", "A fence answered for another serial is reported as a serial mismatch", scanout_fault_stale_fence),
    case("fault-outside-damage", "Corruption written outside any damage is found by comparing the full destination", scanout_fault_outside_damage),
    case("boundary-stated", "The record states what lies outside the guest's view for this backend", scanout_boundary_stated),
];

// ---------------------------------------------------------------------------------------
// scanout: display observation on every target (contract section 20)
// ---------------------------------------------------------------------------------------

fn scanout_mode_set() -> CaseResult {
    let i = fbinfo().cite("18")?;
    ensure((i.width, i.height) == (1280, 800), "18, 20.2", format!("fbinfo reports {}x{}; the contract requires 1280x800 in a GUI suite boot", i.width, i.height))?;
    needs_window("20.1", "the observation record's 1280x800 mode")
}

fn scanout_device_path() -> CaseResult { needs_window("20.1, 20.2", "the record naming this target's backend") }
fn scanout_changing_scene() -> CaseResult { needs_window("20.1", "three successive scenes read back equal to their formulas") }
fn scanout_full_destination() -> CaseResult { needs_window("20.1", "the whole destination read, its last row included") }
fn scanout_binding() -> CaseResult { needs_window("20.1", "the bound resource named in the record") }
fn scanout_completion() -> CaseResult { needs_window("20.1, 20.2", "answered completions for every damaged rectangle") }
fn scanout_atomic_record() -> CaseResult { needs_window("20.1", "pixels and record of one composite serial under continuous commits") }
fn scanout_window_commits() -> CaseResult { needs_window("20.1", "each composed window's commit id in the record") }
fn scanout_format_canonical() -> CaseResult { needs_window("20.1", "RGB and BGR destinations read back as the same canonical pixels") }
fn scanout_root_only() -> CaseResult { needs_window("17.4, 20.1", "EPERM for a non-root, non-compositor SCANOUT_READBACK") }
fn scanout_fault_skipped_transfer() -> CaseResult { needs_window("20.3, 21", "skip-transfer reported as missing completion, then recovery") }
fn scanout_fault_wrong_resource() -> CaseResult { needs_window("20.3, 21", "wrong-resource reported as a binding mismatch") }
fn scanout_fault_stale_fence() -> CaseResult { needs_window("20.3, 21", "stale-fence reported as a serial mismatch") }
fn scanout_fault_outside_damage() -> CaseResult { needs_window("20.3, 21", "corruption outside damage found in the full destination") }
fn scanout_boundary_stated() -> CaseResult { needs_window("20.1, 20.2", "the record's boundary code for this backend") }
