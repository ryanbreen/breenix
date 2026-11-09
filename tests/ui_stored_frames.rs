#[path = "../libs/libuitest/src/messages.rs"] pub mod messages;
#[path = "../libs/libuitest/src/arena.rs"] pub mod arena;
#[path = "../libs/libuitest/src/artifacts.rs"] pub mod artifacts;
extern crate self as libuitest;
extern crate self as breenix_suite;
#[path = "../libs/libuitest/src/outcome.rs"] pub mod outcome;
pub use outcome::{FailureClass, ScenarioOutcome};
#[path = "../libs/libuitest/src/compare.rs"] pub mod compare;
#[path = "../libs/libuitest/src/schema.rs"] pub mod schema;
#[path = "../libs/libuitest/src/formula.rs"] pub mod formula;
#[path = "../libs/libuitest/src/client.rs"] mod client;
pub use client::RecordClient;
#[path = "../libs/libuitest/src/frames.rs"] pub mod frames;
#[path = "ui_frames/formula_pattern.rs"] mod formula_pattern;
#[path = "ui_frames/reference_cases.rs"] pub mod cases;

// Only the syscall boundary is substituted; client and decoder are production code.
#[path = "../libs/libbreenix/src/suite_result.rs"] pub mod result;
pub mod suite {
    pub use crate::result;
    pub fn result_record(line: &str) -> Result<(), String> {
        SENT.lock().unwrap().push(line.to_owned());
        Ok(())
    }
    pub static SENT: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
}

fn root() -> std::path::PathBuf {
    std::path::Path::new(file!()).parent().unwrap().parent().unwrap().join("docs/suites/frames")
}

#[test] fn tampered_pin_precedes_png_decoder() { cases::reference_pin_at(&root()).unwrap(); }
#[test] fn missing_file_names_path() { cases::reference_missing_at(&root()).unwrap(); }
#[test] fn png_roundtrip_and_generated_reference() { cases::png_reference_at(&root()).unwrap(); }

#[test]
fn records_decode_and_account_for_the_exact_capture() {
    use compare::{Format, Frame};
    use frames::{Bindings, Capture};
    let mut definition = cases::definition("frames/frame-record");
    definition["source"] = serde_json::json!({"kind":"window", "window":"pattern"});
    let mut bindings = Bindings::default();
    bindings.window("pattern");
    bindings.checkpoint("after-pattern");
    let pixels = formula::reference(4, 4, formula_pattern::pixel);
    let checks = ["pattern"];
    let mut client = RecordClient::outer("ui-testing", "frames/frame-record", &checks).unwrap();
    suite::SENT.lock().unwrap().clear();
    let capture = Capture { frame: Frame::new(&pixels,4,4,16,Format::Bgrx).unwrap(),
        source: "window:pattern", checkpoint: "after-pattern", commit: 51, epoch: 7, barriers: &[] };
    let verdict = frames::check_stored("ui-testing", &definition, capture, &bindings, &root(), &mut client).unwrap();
    assert!(verdict.comparison.passed());
    let lines = suite::SENT.lock().unwrap().clone();
    assert_eq!(lines.len(), 1);
    let line = &lines[0];
    let def = frames::definition_hash(&definition).unwrap();
    assert!(line.contains(&format!(" def={} ref={} source=window:pattern commit=51 epoch=7 w=4 h=4 ", &def[..8], &cases::PIN[..8])), "{line}");
    assert!(line.contains(&format!("sha256={} result=match", frames::digest(&pixels))));
    let mut decoder = suite::result::Accounting::new("ui-testing", "frames/frame-record", &checks, false).unwrap();
    decoder.accept(line).unwrap();
    assert_eq!(decoder.problem(), None);
    assert!(matches!(client.finish(ScenarioOutcome::pass()), ScenarioOutcome::Pass { records } if records.len() == 1));

    // The failure line must be accepted by the same unchanged decoder and stay failed.
    let mut changed = pixels.clone(); changed[4] = 9;
    let mut client = RecordClient::contained("ui-testing", "frames/frame-record", &checks).unwrap();
    let capture = Capture { frame: Frame::new(&changed,4,4,16,Format::Bgrx).unwrap(),
        source: "window:pattern", checkpoint: "after-pattern", commit: 52, epoch: 8, barriers: &[] };
    let verdict = frames::check_stored("ui-testing", &definition, capture, &bindings, &root(), &mut client).unwrap();
    assert_eq!(verdict.failure_class(), Some(FailureClass::PixelMismatch));
    let outcome = client.finish(ScenarioOutcome::fail(FailureClass::PixelMismatch, "pattern", "planted"));
    let line = &outcome.records()[0];
    assert!(line.contains("result=mismatch compared=16 differing=1 first=1,0 got=#1e0009 want=#1e000a artifacts=none:not-requested"), "{line}");
    let mut decoder = suite::result::Accounting::new("ui-testing", "frames/frame-record", &checks, false).unwrap();
    decoder.accept(line).unwrap();
    assert!(decoder.problem().unwrap().contains("mismatched"));
    // Hash stability is independent of source JSON field order; changing any field binds a new definition.
    let same = schema::from_str(&definition.to_string()).unwrap();
    assert_eq!(frames::definition_hash(&same).unwrap(), def);
    definition["at"]["checkpoint"] = "other".into();
    assert_ne!(frames::definition_hash(&definition).unwrap(), def);
}

fn regenerated_scenario_pins(scenario: &str, stages: &[&str]) {
    let repo = root().parent().unwrap().parent().unwrap().parent().unwrap().to_path_buf();
    let out = std::env::temp_dir().join(format!("ui-refgen-{}-{scenario}", std::process::id()));
    let output = std::process::Command::new("cargo")
        .args(["run", "--quiet", "--release", "--manifest-path"])
        .arg(repo.join("tools/gfx-refgen/Cargo.toml"))
        .args(["--bin", "ui-widgets", "--", scenario])
        .arg(&out)
        .env("CARGO_BUILD_JOBS", "4")
        .output().expect("run host reference generator");
    assert!(output.status.success(), "gfx-refgen failed: {}", String::from_utf8_lossy(&output.stderr));
    assert!(output.stderr.is_empty(), "generator warnings: {}", String::from_utf8_lossy(&output.stderr));
    let provenance: serde_json::Value = serde_json::from_slice(&std::fs::read(root().join("PROVENANCE.json")).unwrap()).unwrap();
    for scale in [1, 2] {
        for stage in stages {
            let name = format!("{scenario}-{stage}-s{scale}.png");
            let path = format!("ui-testing/{name}");
            let rows: Vec<_> = provenance["references"].as_array().unwrap().iter()
                .filter(|r| r["path"] == path).collect();
            assert_eq!(rows.len(), 1, "one pin for {path}");
            let pin = rows[0]["sha256"].as_str().unwrap();
            let regenerated = std::fs::read(out.join(&name)).unwrap();
            let committed = std::fs::read(root().join(&path)).unwrap();
            assert_eq!(frames::digest(&regenerated), pin, "regenerated {path}");
            assert_eq!(regenerated, committed, "committed {path}");
            let image = frames::decode_reference(std::path::Path::new(&path), &committed, pin).unwrap();
            assert_eq!((image.width, image.height), (640 * scale, 400 * scale));
        }
    }
    std::fs::remove_dir_all(out).unwrap();
}

#[test]
fn form_reference_pins_match_regenerated_pngs() {
    regenerated_scenario_pins("form", &["initial", "typed", "checked", "slider", "submitted"]);
}
