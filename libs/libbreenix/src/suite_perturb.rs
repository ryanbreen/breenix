//! Four-phase output check beside the suite verdict (E22 22.2, G29).
use crate::gpu::abi::{screen_crc::{OutputConfig, Record}, Mode};
use std::{string::String, vec::Vec};

pub(super) const NAMES: [&str; 4] = ["perturb-one", "perturb-two", "perturb-three", "perturb-four"];

pub(super) struct Observation {
    pub hash: [u8; 32],
    pub mode: Mode,
    pub binding: Mode,
    pub raw: Vec<u8>,
}
pub(super) trait Driver {
    fn fired(&mut self) -> Result<u64, String>;
    fn arm(&mut self, resource: u64) -> Result<(), String>;
    fn disarm(&mut self) -> Result<(), String>;
    fn observe(&mut self, checkpoint: &str) -> Result<Observation, String>;
}

fn same_config(a: &OutputConfig, b: &OutputConfig) -> bool {
    a.mode == b.mode && a.raster == b.raster && a.viewport == b.viewport
        && a.sor == b.sor && a.rg == b.rg && a.methods == b.methods
}
fn crc(record: &Record) -> (u32, u32) { (record.samples[1].crc, record.samples[1].crc2) }
fn checked(observation: &Observation, suite: &str, case: &str, name: &str) -> Result<Record, String> {
    super::screen::format_crc_record(suite, case, name, observation.hash,
        observation.mode, observation.binding, &observation.raw)?;
    Record::decode(&observation.raw).map_err(String::from)
}
fn same_frame(a: &Observation, b: &Observation, x: &Record, y: &Record) -> bool {
    a.hash == b.hash && (a.mode.width, a.mode.height) == (b.mode.width, b.mode.height)
        && (a.binding.width, a.binding.height) == (b.binding.width, b.binding.height)
        && x.binding_before == y.binding_before
}
fn line(suite: &str, case: &str, phase: usize, observation: &Observation,
    fired: u64, disarmed: bool) -> Result<String, String> {
    let record = checked(observation, suite, case, NAMES[phase - 1])?;
    let screen = super::screen::format_crc_record(suite, case, NAMES[phase - 1],
        observation.hash, observation.mode, observation.binding, &observation.raw)?;
    let fields = screen.trim_end().splitn(6, ' ').nth(5).ok_or("perturb-line")?;
    Ok(std::format!("SUITE {suite} SCREENPERTURB {case} end phase={phase} {fields} resource={} present_fence={} fired={fired} disarmed={}\n",
        record.binding_before.resource, record.binding_before.present_fence, u8::from(disarmed)))
}

/// The observer uses the existing pinned pixels/CRC request. The control is
/// disarmed even when arming or an armed observation reports an error.
pub(super) fn run(suite: &str, case: &str, baseline: &Observation, driver: &mut impl Driver)
    -> Result<(Vec<String>, Vec<Vec<u8>>), String> {
    let base = checked(baseline, suite, case, "end")?;
    let before = driver.fired()?;
    let one = driver.observe(NAMES[0])?;
    let first = checked(&one, suite, case, NAMES[0])?;
    if !same_frame(baseline, &one, &base, &first) || crc(&base) != crc(&first)
        || !same_config(&base.output_config, &first.output_config) {
        return Err("perturb-baseline-moved".into());
    }
    let armed: Result<(Observation, Observation, u64), String> = (|| {
        driver.arm(base.binding_before.resource)?;
        let two = driver.observe(NAMES[1])?;
        let second = checked(&two, suite, case, NAMES[1])?;
        let fired = driver.fired()?;
        if before.checked_add(1) != Some(fired) { return Err("perturb-unfired".into()); }
        let three = driver.observe(NAMES[2])?;
        let third = checked(&three, suite, case, NAMES[2])?;
        if driver.fired()? != fired || !same_frame(&one, &two, &first, &second)
            || !same_frame(&one, &three, &first, &third) || crc(&first).0 == crc(&third).0
            || !same_config(&second.output_config, &third.output_config) {
            return Err("perturb-observation".into());
        }
        Ok((two, three, fired))
    })();
    let disarm = driver.disarm();
    let (two, three, fired) = armed?;
    disarm?;
    let four = driver.observe(NAMES[3])?;
    let fourth = checked(&four, suite, case, NAMES[3])?;
    if driver.fired()? != fired || !same_frame(&one, &four, &first, &fourth)
        || crc(&first) != crc(&fourth) || !same_config(&first.output_config, &fourth.output_config) {
        return Err("perturb-not-restored".into());
    }
    let observations = [one, two, three, four];
    let mut lines = Vec::new();
    for (i, observation) in observations.iter().enumerate() {
        lines.push(line(suite, case, i + 1, observation, if i == 0 {before} else {fired}, i == 0 || i == 3)?);
    }
    Ok((lines, observations.into_iter().map(|v|v.raw).collect()))
}

/// Check the case-to-parent text envelope before emitting it. Raw records have
/// their own SHA-256/length queue and are joined by checkpoint identity on host.
pub(super) fn validate_line(line: &str, suite: &str, case: &str) -> Result<usize, &'static str> {
    let words: Vec<_> = line.split_whitespace().collect();
    if words.len() != 17 || words[..5] != ["SUITE", suite, "SCREENPERTURB", case, "end"] {
        return Err("perturb-side-file");
    }
    let phase = words[5].strip_prefix("phase=").and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| (1..=4).contains(&n)).ok_or("perturb-phase")?;
    let screen = std::format!("SUITE {suite} SCREEN {case} {} {}\n", NAMES[phase - 1], words[6..13].join(" "));
    super::screen::unpack(&super::screen::pack(&screen), suite, case)?;
    for (index, prefix) in [(13,"resource="), (14,"present_fence="), (15,"fired=")] {
        let n = words[index].strip_prefix(prefix).and_then(|s|s.parse::<u64>().ok()).ok_or("perturb-counter")?;
        if index != 15 && n == 0 { return Err("perturb-binding"); }
    }
    if words[16] != if phase == 1 || phase == 4 {"disarmed=1"} else {"disarmed=0"} {
        return Err("perturb-disarm");
    }
    Ok(phase)
}
