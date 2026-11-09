//! Stored UI references and FRAME records (docs/gui/plan.md sections 7.5 and 8.3).
use crate::{
    compare::{self, Format, Frame, Mask, Options, Rect, Tolerance, Verdict},
    schema::{self, Check, Context, Value},
    RecordClient,
};
use libimg::png::Image;
use std::{
    fs,
    path::{Component, Path},
};

pub fn digest(bytes: &[u8]) -> String {
    libpixcheck::sha256(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Compact JSON with recursively sorted object keys; array order is significant.
pub fn definition_hash(value: &Value) -> Result<String, String> {
    fn canonical(value: &Value, out: &mut String) -> Result<(), String> {
        match value {
            Value::Object(object) => {
                out.push('{');
                let mut keys: Vec<_> = object.keys().collect();
                keys.sort_unstable();
                for (i, key) in keys.iter().enumerate() {
                    if i != 0 {
                        out.push(',');
                    }
                    out.push_str(&serde_json::to_string(key).map_err(|e| e.to_string())?);
                    out.push(':');
                    canonical(&object[*key], out)?;
                }
                out.push('}');
            }
            Value::Array(array) => {
                out.push('[');
                for (i, value) in array.iter().enumerate() {
                    if i != 0 {
                        out.push(',');
                    }
                    canonical(value, out)?;
                }
                out.push(']');
            }
            _ => out.push_str(&serde_json::to_string(value).map_err(|e| e.to_string())?),
        }
        Ok(())
    }
    let mut bytes = String::new();
    canonical(value, &mut bytes)?;
    Ok(digest(bytes.as_bytes()))
}

/// Hash exactly the bytes subsequently decoded, with no second path read.
pub fn decode_reference(path: &Path, bytes: &[u8], pin: &str) -> Result<Image, String> {
    if pin.len() != 64 || !pin.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("invalid reference pin: {}", path.display()));
    }
    let got = digest(bytes);
    if !got.eq_ignore_ascii_case(pin) {
        return Err(format!(
            "reference pin mismatch: {} want={pin} got={got}",
            path.display()
        ));
    }
    libimg::png::decode(bytes).map_err(|e| format!("reference PNG {}: {e:?}", path.display()))
}

pub fn load_reference(path: &Path, pin: &str) -> Result<Image, String> {
    let bytes = fs::read(path).map_err(|e| {
        let reason = if e.kind() == std::io::ErrorKind::NotFound {
            "missing reference"
        } else {
            "reference read"
        };
        format!("{reason}: {}: {e}", path.display())
    })?;
    decode_reference(path, &bytes, pin)
}

/// Scenario observations, populated when a window is assigned its key and a step
/// is reached. Finish checks declarations with unexecuted comparisons too.
#[derive(Default)]
pub struct Bindings {
    windows: Vec<String>,
    checkpoints: Vec<String>,
}
impl Bindings {
    pub fn window(&mut self, key: &str) {
        if !self.windows.iter().any(|w| w == key) {
            self.windows.push(key.into());
        }
    }
    pub fn checkpoint(&mut self, name: &str) {
        if !self.checkpoints.iter().any(|c| c == name) {
            self.checkpoints.push(name.into());
        }
    }
    pub fn finish(&self, checks: &[&Check]) -> Result<(), String> {
        for check in checks {
            if let Some(key) = check.source["window"].as_str() {
                if !self.windows.iter().any(|w| w == key) {
                    return Err(format!("check {}: unreached window key {key}", check.check));
                }
            }
            let checkpoint = check.definition["at"]["checkpoint"].as_str().unwrap();
            if !self.checkpoints.iter().any(|c| c == checkpoint) {
                return Err(format!(
                    "check {}: unreached checkpoint {checkpoint}",
                    check.check
                ));
            }
        }
        Ok(())
    }
}

/// The capture supplies the compared pixels and their observation metadata.
pub struct Capture<'a> {
    pub frame: Frame<'a>,
    pub source: &'a str,
    pub checkpoint: &'a str,
    pub commit: u64,
    pub epoch: u64,
    pub barriers: &'a [&'a str],
}

/// Evidence supplied by a session, captured at the check's step.
pub struct ArtifactContext<'a> {
    pub arena: &'a crate::arena::Arena,
    pub input_log: &'a [u8],
    pub writer: &'a mut dyn crate::artifacts::CheckWriter,
    pub save_all: bool,
}

/// Run one stored check and send one record through the existing client. Refused
/// inputs return a named error; the runner's never-ran obligation stays unsatisfied.
pub fn check_stored(
    suite: &str,
    definition: &Value,
    capture: Capture<'_>,
    bindings: &Bindings,
    root: &Path,
    client: &mut RecordClient<'_>,
) -> Result<Verdict, String> {
    check_stored_impl(suite, definition, capture, bindings, root, client, None)
}

pub fn check_stored_with_artifacts(
    suite: &str,
    definition: &Value,
    capture: Capture<'_>,
    bindings: &Bindings,
    root: &Path,
    client: &mut RecordClient<'_>,
    artifacts: &mut ArtifactContext<'_>,
) -> Result<Verdict, String> {
    check_stored_impl(
        suite,
        definition,
        capture,
        bindings,
        root,
        client,
        Some(artifacts),
    )
}

fn check_stored_impl(
    suite: &str,
    definition: &Value,
    capture: Capture<'_>,
    bindings: &Bindings,
    root: &Path,
    client: &mut RecordClient<'_>,
    mut artifacts: Option<&mut ArtifactContext<'_>>,
) -> Result<Verdict, String> {
    let check = schema::validate(
        definition,
        &Context {
            frame: Some((capture.frame.width as u32, capture.frame.height as u32)),
            formulas: &[],
            previous: None,
        },
    )?;
    if definition["kind"] != "stored" {
        return Err(format!("check {}: expected stored reference", check.check));
    }
    bindings.finish(&[&check])?;
    let source = match check.source["kind"].as_str().unwrap() {
        "window" => format!("window:{}", check.source["window"].as_str().unwrap()),
        kind => kind.to_owned(),
    };
    if source != capture.source {
        return Err(format!(
            "check {}: source {source} not captured (got {})",
            check.check, capture.source
        ));
    }
    let checkpoint = definition["at"]["checkpoint"].as_str().unwrap();
    if checkpoint != capture.checkpoint {
        return Err(format!(
            "check {}: checkpoint {checkpoint} not captured (got {})",
            check.check, capture.checkpoint
        ));
    }
    for barrier in definition["at"]["requires"].as_array().unwrap() {
        let barrier = barrier.as_str().unwrap();
        if !capture.barriers.contains(&barrier) {
            return Err(format!(
                "check {}: unreached barrier {barrier}",
                check.check
            ));
        }
    }
    let reference = &definition["reference"];
    let relative = Path::new(reference["path"].as_str().unwrap());
    if relative
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(format!(
            "check {}: reference path must be relative: {}",
            check.check,
            relative.display()
        ));
    }
    let pin = reference["sha256"].as_str().unwrap();
    let image = load_reference(&root.join(relative), pin)?;
    let expected = Frame::new(
        &image.pixels,
        image.width as usize,
        image.height as usize,
        image.width as usize * 4,
        Format::Bgrx,
    )
    .map_err(|e| format!("reference frame: {e:?}"))?;
    let rect = |v: &Value| Rect {
        x: v["x"].as_u64().unwrap() as usize,
        y: v["y"].as_u64().unwrap() as usize,
        w: v["w"].as_u64().unwrap() as usize,
        h: v["h"].as_u64().unwrap() as usize,
    };
    let regions: Vec<_> = definition["regions"]
        .as_array()
        .unwrap()
        .iter()
        .map(rect)
        .collect();
    let masks: Vec<_> = definition["mask"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| Mask {
            rect: rect(&m["rect"]),
            reason: m["reason"].as_str().unwrap(),
        })
        .collect();
    let tol = &definition["tolerance"];
    let options = Options {
        regions: &regions,
        masks: &masks,
        tolerance: Tolerance {
            channel: tol["channel"].as_u64().unwrap() as u8,
            max_differing: tol["maxDiffering"].as_u64().unwrap() as usize,
        },
        reason: tol["reason"].as_str().unwrap_or(""),
    };
    let verdict = compare::compare(expected, capture.frame, options)
        .map_err(|e| format!("check {}: {e:?}", check.check))?;
    let saved = artifacts
        .as_mut()
        .filter(|context| !verdict.comparison.passed() || context.save_all)
        .map(|context| {
            // Publication completes before the FRAME record, and hence before CASE.
            let result = crate::artifacts::save_check(
                context.writer,
                context.arena,
                &check.check,
                capture.commit,
                expected,
                capture.frame,
                options,
                context.input_log,
            );
            match result {
                Ok(()) => {
                    let dropped = !verdict.comparison.passed()
                        && context.writer.prepare_retention().ok().flatten() == Some(true);
                    if dropped {
                        "artifacts=partial:screen-dropped".into()
                    } else {
                        format!(
                            "artifacts=complete path={}",
                            context.writer.path().display()
                        )
                    }
                }
                Err(error) => format!("artifacts={}", error.record()),
            }
        });
    let def = definition_hash(definition)?;
    let hash: String = capture
        .frame
        .sha256()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let mut line = format!("SUITE {suite} FRAME {} {} def={} ref={} source={} commit={} epoch={} w={} h={} sha256={hash} result={}",
        check.case, check.check, &def[..8], pin[..8].to_ascii_lowercase(), source, capture.commit, capture.epoch,
        capture.frame.width, capture.frame.height, if verdict.comparison.passed() { "match" } else { "mismatch" });
    if !verdict.comparison.passed() {
        let first = verdict
            .comparison
            .first
            .ok_or("mismatch without first pixel")?;
        line.push_str(&format!(
            " compared={} differing={} {}",
            verdict.comparison.compared,
            verdict.comparison.differing,
            crate::messages::pixel_details(first.x, first.y, first.actual, first.expected)
        ));
    }
    if let Some(saved) = saved {
        line.push(' ');
        line.push_str(&saved);
    } else if !verdict.comparison.passed() {
        line.push_str(" artifacts=none:not-requested");
    }
    client.record(&line)?;
    Ok(verdict)
}
