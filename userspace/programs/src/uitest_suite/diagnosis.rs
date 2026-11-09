//! Local check evidence; records and planted failures stay in the inner executor.
use breenix_suite::suite::CaseResult;
use libuitest::{
    arena::{Slot, SlotRead},
    capture, capture_marker, classify,
    frames::Bindings,
    input::{self, Keyboard, KeyboardBackend, LivePointer, Target},
    Classification, Expected, FailureClass, RecordClient, ScenarioOutcome, ScenarioSpec, Session,
};
use std::{fs, path::Path};

pub fn case_death() -> CaseResult {
    super::runner::collection_evidence("collection-death", "crash")
}
fn require(ok: bool, message: impl Into<String>) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(message.into())
    }
}
fn inner(case: &str, which: u8, client: &mut RecordClient<'_>) -> Result<ScenarioOutcome, String> {
    let mut session = Session::desktop()?;
    let result: Result<ScenarioOutcome, String> = (|| {
        let mut serials = Vec::new();
        if which == 3 {
            session.launch_app(
                &[
                    b"/usr/libexec/breenix/uitest-app_test",
                    b"artifact input",
                    b"--widgets",
                ],
                0,
            )?;
            let id = session.wait_window("artifact input")?.id;
            let mut live = LivePointer::new(&mut session)?;
            live.watch(id)?;
            live.focus(&Target::new(id, "entry"))?;
            let mut keyboard = Keyboard::new(live);
            keyboard.type_text(id, "aZλ")?;
            let live = keyboard.into_backend();
            serials = live.serials().to_vec();
            require(!serials.is_empty(), "artifact input script sent no serials")?;
            input::dispositions(&serials)?;
        }
        let mut app = super::frames::App::launch(&mut session)?;
        let committed = app.command("create")?;
        session.wait_window("capture races")?;
        let record = capture::window(session.arena_mut(), "pixels", committed.id)?;
        require(
            record.commit == committed.generation.commit,
            "artifact capture names a later commit",
        )?;
        // This reference comes from the independently reconstructed producer pattern.
        let mut want = capture_marker::frame(
            committed.width as usize,
            committed.height as usize,
            committed.generation,
        )?;
        let mismatch = which != 1 && which != 2;
        if mismatch {
            let last = want.len() - 4;
            want[last] ^= 0xff;
        }
        let reference = libimg_encode::encode(&libimg::png::Image {
            width: committed.width,
            height: committed.height,
            pixels: want,
        })
        .map_err(|e| format!("reference encode: {e:?}"))?;
        let root = std::path::PathBuf::from(session.config_root());
        fs::write(root.join("reference.png"), &reference).map_err(|e| e.to_string())?;
        let definition = serde_json::json!({"version":1,"case":case,"check":"pixels","kind":"stored",
            "source":{"kind":"window","window":"pattern"},"at":{"checkpoint":"saved","requires":[]},
            "reference":{"path":"reference.png","sha256":libuitest::frames::digest(&reference)},
            "regions":[{"name":"all","x":0,"y":0,"w":committed.width,"h":committed.height}],
            "mask":[],"tolerance":{"channel":0,"maxDiffering":0}});
        let mut bindings = Bindings::default();
        bindings.window("pattern");
        bindings.checkpoint("saved");
        let verdict =
            session.check_stored("ui-testing", &definition, &bindings, &root, &[], client)?;
        require(
            verdict.comparison.passed() != mismatch,
            "artifact scenario comparison has wrong result",
        )?;
        let path = Path::new("/var/uitest/ui-testing").join(case);
        if which == 1 {
            require(!path.exists(), "passing check created artifacts")?;
        } else {
            for file in [
                "pixels.got.png".to_string(),
                "pixels.diff.png".into(),
                format!("tree-{}.txt", committed.id),
                "input.log".into(),
                "screen.png".into(),
                "screen.record".into(),
            ] {
                require(
                    path.join(&file).is_file(),
                    format!("artifact missing {file}"),
                )?;
            }
            require(
                fs::read_dir(&path)
                    .map_err(|e| e.to_string())?
                    .all(|p| p.is_ok_and(|p| !p.file_name().to_string_lossy().ends_with(".tmp"))),
                "partial artifact left after publication",
            )?;
            let SlotRead::Final { bytes: pixels, .. } = session.arena_mut().read(Slot::Frame)?
            else {
                return Err("frame slot not final".into());
            };
            let image = libimg::png::decode(
                &fs::read(path.join("pixels.got.png")).map_err(|e| e.to_string())?,
            )
            .map_err(|e| format!("got PNG: {e:?}"))?;
            require(
                image.pixels == pixels,
                "saved got differs from compared capture",
            )?;
            let SlotRead::Final { bytes: tree, .. } = session.arena_mut().read(Slot::Tree)? else {
                return Err("tree slot not final".into());
            };
            let end = tree.iter().position(|&b| b == 0).unwrap_or(tree.len());
            require(
                fs::read(path.join(format!("tree-{}.txt", committed.id)))
                    .map_err(|e| e.to_string())?
                    == tree[..end],
                "saved tree differs from compared commit",
            )?;
            if which == 3 {
                let log = fs::read_to_string(path.join("input.log")).map_err(|e| e.to_string())?;
                require(
                    input::first_missing_disposition(&log, &serials)?.is_none(),
                    "saved log lacks a script disposition",
                )?;
                for serial in serials {
                    require(
                        input::ledger_entry(&log, serial)?.is_some(),
                        format!("saved log missing serial {serial}"),
                    )?;
                }
            }
            if which == 4 {
                let SlotRead::Final { record, bytes } = session.arena_mut().read(Slot::Screen)?
                else {
                    return Err("screen slot not final".into());
                };
                let image = libimg::png::decode(
                    &fs::read(path.join("screen.png")).map_err(|e| e.to_string())?,
                )
                .map_err(|e| format!("screen PNG: {e:?}"))?;
                require(
                    (image.width, image.height) == (record.width, record.height),
                    "saved screen size changed",
                )?;
                let actual = libuitest::compare::Frame::new(
                    &bytes,
                    record.width as usize,
                    record.height as usize,
                    record.width as usize * 4,
                    libuitest::compare::Format::Bgrx,
                )
                .map_err(|e| format!("screen frame: {e:?}"))?;
                require(
                    image.pixels == actual.canonical(),
                    "saved screen differs from failure-step readback",
                )?;
                require(
                    fs::read(path.join("screen.record")).map_err(|e| e.to_string())?
                        == record.observation,
                    "saved screen readback record changed",
                )?;
            }
        }
        app.command("quit")?;
        session.wait(app.pid)?;
        if mismatch {
            let first = verdict
                .comparison
                .first
                .ok_or("mismatch lacks first pixel")?;
            Ok(ScenarioOutcome::fail(
                FailureClass::PixelMismatch,
                "pixels",
                &format!(
                    "first={},{} got={:?} want={:?}",
                    first.x, first.y, first.actual, first.expected
                ),
            ))
        } else {
            Ok(ScenarioOutcome::pass())
        }
    })();
    let teardown = session.teardown();
    let outcome = result?;
    teardown?;
    Ok(outcome)
}
fn run(id: &str, which: u8) -> Result<(), String> {
    let case = format!("diagnosis/{id}");
    let path = Path::new("/var/uitest/ui-testing").join(&case);
    if path.exists() {
        fs::remove_dir_all(&path).map_err(|e| e.to_string())?;
    }
    // Exercise the same switch that the runner and local writer consult.
    let switch = Path::new("/etc/breenix/uitest-save");
    let previous = fs::read(switch).ok();
    if which == 2 {
        fs::write(switch, b"all\n").map_err(|e| e.to_string())?;
    } else if switch.exists() {
        fs::remove_file(switch).map_err(|e| e.to_string())?;
    }
    let outcome = libuitest::run_scenario(ScenarioSpec {
        suite: "ui-testing",
        case: &case,
        checks: &["pixels"],
        limit_ms: 8000,
        run: &mut |client| {
            inner(&case, which, client).unwrap_or_else(|e| {
                ScenarioOutcome::fail(FailureClass::Infrastructure, "artifact-scenario", &e)
            })
        },
    });
    match previous {
        Some(bytes) => fs::write(switch, bytes).map_err(|e| e.to_string())?,
        None => {
            if switch.exists() {
                fs::remove_file(switch).map_err(|e| e.to_string())?;
            }
        }
    }
    let expected = if which == 1 || which == 2 {
        Expected::Pass
    } else {
        Expected::Fault {
            class: FailureClass::PixelMismatch,
            operation: "pixels",
            evidence: "first=319,199 got=[48, 32, 24, 255] want=[207, 32, 24, 255]",
        }
    };
    require(
        matches!(
            classify(&expected, &outcome),
            Classification::CleanPass | Classification::ExpectedFault
        ),
        format!("artifact scenario: {outcome:?}"),
    )?;
    let record = outcome
        .records()
        .first()
        .ok_or("artifact scenario has no FRAME")?;
    if which != 1 {
        require(
            record.ends_with(&format!("artifacts=complete path={}", path.display())),
            format!("artifact FRAME path: {record}"),
        )?;
    } else {
        require(
            !path.exists(),
            "passing inner case saved evidence at teardown",
        )?;
    }
    Ok(())
}
pub fn artifacts_on_mismatch() -> CaseResult {
    run("artifacts-on-mismatch", 0).map_err(Into::into)
}
pub fn nothing_on_pass() -> CaseResult {
    run("nothing-on-pass", 1).map_err(Into::into)
}
pub fn save_all() -> CaseResult {
    run("save-all", 2).map_err(Into::into)
}
pub fn input_log() -> CaseResult {
    run("input-log", 3).map_err(Into::into)
}
pub fn screen_saved() -> CaseResult {
    run("screen-saved", 4).map_err(Into::into)
}


pub fn app_death() -> CaseResult {
    let path = Path::new("/var/uitest/runner-fixture/fixture/app-death");
    if path.exists() { fs::remove_dir_all(path).map_err(|e| e.to_string())?; }
    let (transcript, exit) = super::runner::fixture("app-death")?;
    require(exit == 1 && transcript.contains("app-death witness"),
        format!("app death fixture did not reach its witness: {transcript}"))?;
    let collect = transcript.lines().find(|l| l.starts_with("SUITE runner-fixture COLLECT fixture/app-death "))
        .ok_or("app death collection record missing")?;
    require(collect.contains("remnants=1 artifacts=complete"), format!("app death collection: {collect}"))?;
    let collect_at = transcript.find(collect).ok_or("collection position missing")?;
    let case_at = transcript.find("CASE fixture/app-death FAIL").ok_or("fixture failure missing")?;
    require(collect_at < case_at, "app death CASE preceded collection")?;
    let value = |name: &str| -> Result<u64, String> {
        let prefix = format!("SUITE runner-fixture VALUE fixture/app-death {name}=");
        let mut lines = transcript.lines().filter_map(|line| line.strip_prefix(&prefix));
        let number = lines.next().and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.strip_suffix("count")).ok_or_else(|| format!("missing app death oracle {name}"))?;
        require(lines.next().is_none(), format!("duplicate app death oracle {name}"))?;
        number.parse::<u64>().map_err(|e| e.to_string())
    };
    let (id, commit, producer, width, height) = (value("remnant-window")?, value("remnant-commit")?,
        value("remnant-producer")?, value("remnant-width")?, value("remnant-height")?);
    let generation = capture_marker::Generation { commit, producer: u32::try_from(producer).map_err(|e| e.to_string())? };
    let stem = format!("remnant-{id}-{commit}");
    let image = libimg::png::decode(&fs::read(path.join(format!("{stem}.png"))).map_err(|e| e.to_string())?)
        .map_err(|e| format!("remnant PNG: {e:?}"))?;
    require(u64::from(image.width) == width && u64::from(image.height) == height,
        "remnant dimensions differ from final commit")?;
    require(image.pixels == capture_marker::frame(width as usize, height as usize, generation)?,
        "saved remnant pixels differ from final commit")?;
    let tree = fs::read(path.join(format!("{stem}.tree"))).map_err(|e| e.to_string())?;
    require(capture::root_marker(&tree)? == capture_marker::encode(generation),
        "saved remnant tree differs from final commit")?;
    let record = fs::read_to_string(path.join(format!("{stem}.record"))).map_err(|e| e.to_string())?;
    require(record.contains(&format!("window={id} ")) && record.contains(&format!("commit={commit} ")),
        "saved remnant record differs from final commit")?;
    let manifest = fs::read_to_string(path.join("collection.txt")).map_err(|e| e.to_string())?;
    for suffix in ["png", "tree", "record"] {
        require(manifest.contains(&format!("file={stem}.{suffix} ")), "remnant absent from synced collection")?;
    }
    Ok(())
}
