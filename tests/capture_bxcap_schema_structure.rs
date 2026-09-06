//! The `BXCAP v1` oracle: a minimal decoder, run over committed self-test
//! serials.
//!
//! This is PR-3's red-to-green leg in the form the plan asks for. `main` has
//! no `[BXCAP:` bytes at all, so every assertion below is red there for the
//! trivial reason that there is nothing to decode; what makes the suite
//! worth keeping afterwards is that it is a real decoder, and it fails on a
//! capture that is malformed as well as on one that is missing.
//!
//! # What it decodes, and what that pins
//!
//! * `BEGIN`/`END` bracketing. A `BEGIN` with no `END` is the definition of
//!   a truncated capture; this suite rejects it rather than scoring the
//!   fragment.
//! * The version gate. `v=` must be present on BOTH bracket lines and must
//!   be a version this decoder knows. An unknown major version is REFUSED,
//!   not best-effort decoded -- that is the whole point of carrying a
//!   version field.
//! * `records=`, against the well-formed records the decoder could actually
//!   parse. The emitter does not count a record its byte budget cut, so
//!   these two numbers have to agree exactly.
//! * The honesty contract on `THR`: the section is either emitted, or its
//!   refusal is stated as `[BXCAP:NOTE sched_lock_held]` AND its bit is set
//!   in `sections_skipped=`. Silence is not one of the options.
//! * `verdict=` against `sections_skipped=` and `truncated=`, so the
//!   summary word cannot disagree with the accounting beside it.
//! * The byte bound, against `BXCAP_BUDGET_BYTES` read out of
//!   `kernel/src/capture/record.rs` -- both cfg'd values, chosen per fixture
//!   by the features its provenance header names.
//!
//! # Why the fixtures are committed serials, not a live boot
//!
//! `--features capture_selftest` is not built by any gate (see
//! `kernel/Cargo.toml`), so there is no gate serial for this suite to score.
//! Committing the real serials makes the oracle deterministic and lets a
//! reader see exactly what the emitter produced. Each fixture carries a
//! provenance header naming the gate, host, branch, base and features.
//!
//! # Anti-vacuity
//!
//! Every assertion body is reachable from a `#[should_panic]` leg that
//! damages a fixture in memory -- strip the version, drop the `END`, delete
//! a section, decrement `records=` -- and asserts the decoder rejects it.
//! Without those, a decoder that returned early would read as green.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

const SERIAL_DIR: &str = "docs/planning/green-program/failure-capture/serials/pr3";
const RECORD_SOURCE: &str = "kernel/src/capture/record.rs";

/// The only schema major version this decoder understands.
const KNOWN_VERSION: u64 = 1;

/// `sections_skipped` bit for `THR`, from `kernel/src/capture/sections.rs`'s
/// `SECTION_THR`. Cross-checked against that source below so the two cannot
/// drift apart silently.
const THR_BIT: u64 = 1 << 5;

fn repo_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn read(rel: &str) -> String {
    let full = repo_path(rel);
    fs::read_to_string(&full)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", full.display()))
}

// ---------------------------------------------------------------------------
// The decoder
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Record {
    token: String,
    fields: BTreeMap<String, String>,
    /// `NOTE` bodies are free-form, so they are kept whole as well.
    body: String,
}

impl Record {
    fn u64(&self, key: &str) -> u64 {
        let raw = self.fields.get(key).unwrap_or_else(|| {
            panic!(
                "BXCAP record `{}` has no `{key}=` field: {:?}",
                self.token, self.fields
            )
        });
        parse_scalar(raw).unwrap_or_else(|| {
            panic!(
                "BXCAP `{}` field `{key}={raw}` is neither decimal nor 0x-hex",
                self.token
            )
        })
    }

    fn text(&self, key: &str) -> &str {
        self.fields
            .get(key)
            .unwrap_or_else(|| panic!("BXCAP record `{}` has no `{key}=` field", self.token))
    }
}

fn parse_scalar(raw: &str) -> Option<u64> {
    if let Some(hex) = raw.strip_prefix("0x") {
        u64::from_str_radix(hex, 16).ok()
    } else {
        raw.parse::<u64>().ok()
    }
}

/// Decode one line into a record, or `None` if the line is not a well-formed
/// `[BXCAP:...]` record.
///
/// "Well-formed" is deliberately strict: the line must start with `[BXCAP:`
/// and end with `]`. A record the emitter's byte budget cut mid-line has no
/// closing `]`, so it is not decoded and not counted -- which is exactly the
/// rule `records=` is written against.
fn decode_line(line: &str) -> Option<Record> {
    let line = line.trim_end_matches(['\r', '\n']);
    let rest = line.strip_prefix("[BXCAP:")?;
    let inner = rest.strip_suffix(']')?;
    if inner.contains(']') || inner.contains('[') {
        return None;
    }
    let mut parts = inner.splitn(2, ' ');
    let token = parts.next()?.to_string();
    if token.is_empty() || !token.chars().all(|c| c.is_ascii_uppercase()) {
        return None;
    }
    let body = parts.next().unwrap_or("").to_string();

    let mut fields = BTreeMap::new();
    if token != "NOTE" {
        for pair in body.split(' ').filter(|s| !s.is_empty()) {
            let mut kv = pair.splitn(2, '=');
            let key = kv.next().unwrap_or("");
            let value = match kv.next() {
                Some(value) => value,
                // The schema is `TOKEN key=value` throughout, NOTE excepted.
                // A bare word anywhere else is malformed, not ignorable.
                None => panic!("BXCAP `{token}` record has a bare `{key}` where a key=value was required: {line}"),
            };
            assert!(
                !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                "BXCAP `{token}` record has a malformed key `{key}`: {line}"
            );
            assert!(
                !value.is_empty(),
                "BXCAP `{token}` record has an empty value for `{key}`: {line}"
            );
            fields.insert(key.to_string(), value.to_string());
        }
    }
    Some(Record {
        token,
        fields,
        body,
    })
}

#[derive(Debug)]
struct Capture {
    begin: Record,
    end: Record,
    /// Every well-formed record from `BEGIN` up to but NOT including `END`.
    records: Vec<Record>,
    /// Lines between the brackets that did not decode. At most one, and only
    /// when the byte budget cut a record.
    undecodable: Vec<String>,
}

/// Every `[BXCAP:` line in `serial` must be CRLF-terminated.
///
/// The schema names `\r\n` as the record terminator and the emitter writes
/// it, so the fixture has to show it. `.gitattributes` marks these files
/// `-text` for exactly this reason: a CRLF normalisation would delete the
/// byte this assertion is about and leave the suite pinning a property its
/// own fixture no longer demonstrates.
fn assert_records_are_crlf_terminated(name: &str, serial: &str) {
    let mut checked = 0;
    for (i, line) in serial.split('\n').enumerate() {
        if !line.trim_start().starts_with("[BXCAP:") {
            continue;
        }
        checked += 1;
        assert!(
            line.ends_with('\r'),
            "{name}:{}: BXCAP record is not CRLF-terminated: {line:?}",
            i + 1
        );
    }
    assert!(
        checked > 0,
        "{name}: no [BXCAP: records at all; there is nothing here to check"
    );
}

/// Decode the single capture in `serial`, refusing anything that is not one
/// complete, bracketed, known-version capture.
fn decode_capture(serial: &str) -> Capture {
    let lines: Vec<&str> = serial.lines().collect();

    let begin_positions: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.trim_start().starts_with("[BXCAP:BEGIN"))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        begin_positions.len(),
        1,
        "expected exactly one [BXCAP:BEGIN in this serial, found {}",
        begin_positions.len()
    );
    let begin_at = begin_positions[0];

    let end_at = lines
        .iter()
        .enumerate()
        .skip(begin_at)
        .find(|(_, line)| line.trim_start().starts_with("[BXCAP:END"))
        .map(|(i, _)| i)
        .unwrap_or_else(|| {
            panic!(
                "truncated capture: a [BXCAP:BEGIN at line {} with no matching [BXCAP:END. \
                 A BEGIN without an END is the schema's definition of a capture that was cut off.",
                begin_at + 1
            )
        });

    let begin = decode_line(lines[begin_at])
        .unwrap_or_else(|| panic!("malformed BEGIN line: {}", lines[begin_at]));
    let end = decode_line(lines[end_at])
        .unwrap_or_else(|| panic!("malformed END line: {}", lines[end_at]));

    // The version gate, before anything else is believed.
    for (which, record) in [("BEGIN", &begin), ("END", &end)] {
        let version = record.fields.get("v").unwrap_or_else(|| {
            panic!(
                "BXCAP {which} carries no `v=` field. A decoder cannot know which schema \
                 it is looking at, so it refuses rather than guessing: {}",
                record.body
            )
        });
        let version = parse_scalar(version)
            .unwrap_or_else(|| panic!("BXCAP {which} has a non-numeric `v={version}`"));
        assert_eq!(
            version, KNOWN_VERSION,
            "BXCAP {which} declares schema version {version}; this decoder knows only \
             v={KNOWN_VERSION} and refuses an unknown major version rather than mis-decoding it"
        );
    }

    let mut records = Vec::new();
    let mut undecodable = Vec::new();
    for line in &lines[begin_at..end_at] {
        match decode_line(line) {
            Some(record) => records.push(record),
            None => {
                if line.contains("[BXCAP:") {
                    undecodable.push((*line).to_string());
                }
            }
        }
    }

    Capture {
        begin,
        end,
        records,
        undecodable,
    }
}

/// The contract every capture must satisfy, whatever its outcome.
fn assert_capture_contract(name: &str, capture: &Capture) {
    let seq_begin = capture.begin.u64("seq");
    let seq_end = capture.end.u64("seq");
    assert_eq!(
        seq_begin, seq_end,
        "{name}: BEGIN seq={seq_begin} does not match END seq={seq_end}; \
         the bracket does not belong to one capture"
    );
    assert_eq!(
        capture.begin.text("edge"),
        capture.end.text("edge"),
        "{name}: BEGIN and END disagree about which edge fired"
    );

    // `records=` against what actually decoded. The emitter counts a record
    // only after it has written the record's terminator, so a budget-cut
    // line is in neither number.
    let declared = capture.end.u64("records");
    assert_eq!(
        declared,
        capture.records.len() as u64,
        "{name}: END declares records={declared} but {} well-formed records decode \
         between BEGIN and END (undecodable fragments: {:?})",
        capture.records.len(),
        capture.undecodable
    );

    let truncated = capture.end.u64("truncated");
    assert!(
        truncated <= 1,
        "{name}: truncated= must be 0 or 1, got {truncated}"
    );
    assert!(
        capture.undecodable.len() <= 1,
        "{name}: {} fragments failed to decode; the byte budget can cut at most the one \
         record in flight when it runs out: {:?}",
        capture.undecodable.len(),
        capture.undecodable
    );
    if truncated == 0 {
        assert!(
            capture.undecodable.is_empty(),
            "{name}: END says truncated=0 but a record did not decode: {:?}",
            capture.undecodable
        );
    }

    // verdict= must agree with the accounting beside it.
    let skipped = capture.end.u64("sections_skipped");
    let verdict = capture.end.text("verdict");
    let expected = if skipped == 0 && truncated == 0 {
        "complete"
    } else {
        "partial"
    };
    assert_eq!(
        verdict, expected,
        "{name}: END says verdict={verdict} with sections_skipped={skipped:#x} \
         truncated={truncated}; the summary word disagrees with its own accounting"
    );

    // The THR honesty contract: emitted, or refused out loud.
    let has_thr = capture.records.iter().any(|r| r.token == "THR");
    let has_note = capture
        .records
        .iter()
        .any(|r| r.token == "NOTE" && r.body.contains("sched_lock_held"));
    let thr_skipped = skipped & THR_BIT != 0;
    if has_thr {
        assert!(
            !thr_skipped,
            "{name}: THR rows are present but its bit is set in sections_skipped={skipped:#x}"
        );
    } else if truncated == 0 {
        assert!(
            has_note && thr_skipped,
            "{name}: no THR rows, and the refusal is not stated. A capture that could not \
             read the scheduler must say [BXCAP:NOTE sched_lock_held] AND set THR's bit in \
             sections_skipped (got note={has_note} skipped={skipped:#x})"
        );
    } else {
        // A truncated capture never reached THR at all, and `truncated=1`
        // with THR's bit set is that explanation. Demanding the refusal note
        // here would demand a section the budget stopped before.
        assert!(
            thr_skipped,
            "{name}: truncated capture with no THR rows must still set THR's bit in \
             sections_skipped, got {skipped:#x}"
        );
    }
    let thr_rows = capture.records.iter().filter(|r| r.token == "THR").count();
    assert!(
        thr_rows == 0 || thr_rows == 8,
        "{name}: THR emits one row per scheduler CPU slot or none at all, got {thr_rows}"
    );
}

/// Sections a capture that ran to completion must carry.
const REQUIRED_SECTIONS: [&str; 5] = ["EDGE", "CPU", "EV", "CNT", "RING"];

fn assert_untruncated_sections(name: &str, capture: &Capture) {
    for token in REQUIRED_SECTIONS {
        assert!(
            capture.records.iter().any(|r| r.token == token),
            "{name}: section {token} is missing from a capture that reports truncated=0"
        );
    }
}

/// `BXCAP_BUDGET_BYTES` for the two cfg arms, read out of the emitter's own
/// source so this suite cannot pin a number the kernel no longer uses.
fn budget_bytes(tiny: bool) -> u64 {
    let source = read(RECORD_SOURCE);
    let wanted = if tiny {
        "capture_selftest_tiny_budget"
    } else {
        "not(feature = \"capture_selftest_tiny_budget\")"
    };
    let mut pending = false;
    for line in source.lines() {
        if line.contains("#[cfg(") && line.contains(wanted) {
            pending = true;
            continue;
        }
        if pending && line.contains("pub const BXCAP_BUDGET_BYTES: u32 =") {
            let value = line
                .split('=')
                .nth(1)
                .and_then(|rhs| rhs.trim().trim_end_matches(';').parse::<u64>().ok());
            return value.unwrap_or_else(|| panic!("could not parse BXCAP_BUDGET_BYTES from: {line}"));
        }
        if pending && !line.trim().is_empty() && !line.trim_start().starts_with("//") {
            pending = false;
        }
    }
    panic!("no BXCAP_BUDGET_BYTES for the `{wanted}` arm in {RECORD_SOURCE}");
}

/// The emitter's stated bound: section content is capped by the budget, and
/// the two bracket lines plus one line terminator sit outside it.
fn assert_byte_bound(name: &str, capture: &Capture, tiny: bool) {
    let budget = budget_bytes(tiny);
    let begin_len = capture.begin.body.len() as u64 + "[BXCAP:BEGIN ]\r\n".len() as u64;
    let bound = budget + begin_len + 2;
    let bytes = capture.end.u64("bytes");
    assert!(
        bytes <= bound,
        "{name}: END reports bytes={bytes}, over the emitter's stated bound of {bound} \
         (BXCAP_BUDGET_BYTES={budget} of section content + the BEGIN line + one terminator). \
         Either the budget stopped being enforced or the bound is wrong."
    );
}

// ---------------------------------------------------------------------------
// The fixtures
// ---------------------------------------------------------------------------

fn fixture(name: &str) -> String {
    read(&format!("{SERIAL_DIR}/{name}"))
}

#[test]
fn thr_bit_matches_the_emitters_own_section_numbering() {
    let sections = read("kernel/src/capture/sections.rs");
    assert!(
        sections.contains("pub const SECTION_THR: u32 = 5;"),
        "SECTION_THR moved; THR_BIT in this suite is derived from it and would now \
         be checking the wrong bit of sections_skipped"
    );
}

#[test]
fn selftest_capture_with_the_scheduler_read_granted_is_complete() {
    let serial = fixture("aarch64-selftest-complete.txt");
    assert_records_are_crlf_terminated("complete", &serial);
    let capture = decode_capture(&serial);
    assert_capture_contract("complete", &capture);
    assert_untruncated_sections("complete", &capture);
    assert_byte_bound("complete", &capture, false);

    assert_eq!(capture.end.text("verdict"), "complete");
    assert_eq!(capture.end.u64("sections_skipped"), 0);
    assert_eq!(capture.end.u64("truncated"), 0);
    assert_eq!(capture.begin.text("edge"), "SELFTEST");
    assert_eq!(capture.begin.text("arch"), "aarch64");

    // The BEGIN header's own fields have to be readable, not just present.
    assert!(capture.begin.u64("tsfreq") > 0, "BEGIN tsfreq must be nonzero");
    assert!(capture.begin.u64("uptime_ms") > 0, "BEGIN uptime_ms must be nonzero");

    // The capturing CPU's row, and the ring accounting that says how much of
    // the boot the EV tail actually covers.
    let cpu_rows: Vec<&Record> = capture.records.iter().filter(|r| r.token == "CPU").collect();
    assert_eq!(cpu_rows.len(), 1, "one CPU row, for the capturing CPU");
    assert_eq!(cpu_rows[0].text("q"), "exact");
    let ring: Vec<&Record> = capture.records.iter().filter(|r| r.token == "RING").collect();
    assert_eq!(ring.len(), 1);
    assert!(ring[0].fields.contains_key("dropped"));
    assert!(ring[0].fields.contains_key("span_us"));
}

#[test]
fn selftest_capture_with_the_scheduler_read_refused_still_reports_everything_else() {
    let serial = fixture("aarch64-selftest-sched-lock-held.txt");
    let capture = decode_capture(&serial);
    assert_capture_contract("sched-lock-held", &capture);
    // The point of the section order: a refused THR costs THR and nothing
    // above it.
    assert_untruncated_sections("sched-lock-held", &capture);
    assert_byte_bound("sched-lock-held", &capture, false);

    assert_eq!(capture.end.u64("truncated"), 0);
    assert_eq!(capture.end.u64("sections_skipped"), THR_BIT);
    assert_eq!(capture.end.text("verdict"), "partial");
    assert!(
        capture
            .records
            .iter()
            .any(|r| r.token == "NOTE" && r.body.trim() == "sched_lock_held"),
        "the refusal must be stated verbatim as `[BXCAP:NOTE sched_lock_held]`"
    );
}

#[test]
fn the_byte_budget_binds_and_the_end_line_still_lands() {
    let serial = fixture("aarch64-selftest-tiny-budget.txt");
    let capture = decode_capture(&serial);
    assert_capture_contract("tiny-budget", &capture);
    assert_byte_bound("tiny-budget", &capture, true);

    assert_eq!(
        capture.end.u64("truncated"),
        1,
        "a capture built with capture_selftest_tiny_budget cannot fit; if it reports \
         truncated=0 the budget is not being enforced"
    );
    assert!(
        capture.end.u64("sections_skipped") != 0,
        "a truncated capture must name the sections it never reached"
    );
    // The bracket survives the cut: this is the property PR-5's drain will
    // be measured against.
    assert_eq!(capture.end.text("edge"), "SELFTEST");
    assert!(
        capture.records.iter().any(|r| r.token == "EDGE"),
        "the cheapest sections must still have been emitted before the budget ran out"
    );
}

#[test]
fn every_committed_fixture_decodes() {
    let dir = repo_path(SERIAL_DIR);
    let mut checked = 0;
    for entry in fs::read_dir(&dir).expect("serials/pr3 must exist") {
        let path = entry.expect("readable dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("txt") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let serial = fs::read_to_string(&path).expect("readable fixture");
        assert_records_are_crlf_terminated(&name, &serial);
        let capture = decode_capture(&serial);
        assert_capture_contract(&name, &capture);
        checked += 1;
    }
    // Census-anchored, not a literal list: a fixture added to the directory
    // is decoded the moment it lands, and an empty directory is a failure
    // rather than a silent pass.
    assert!(
        checked >= 3,
        "expected at least the three PR-3 fixtures under {SERIAL_DIR}, decoded {checked}"
    );
}

// ---------------------------------------------------------------------------
// Anti-vacuity: each leg damages a fixture in memory and asserts the decoder
// rejects it. Without these, a decoder that returned early would be green.
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "no `v=` field")]
fn a_capture_with_no_version_field_is_refused() {
    let serial = fixture("aarch64-selftest-complete.txt").replace("v=1 ", "");
    let capture = decode_capture(&serial);
    assert_capture_contract("no-version", &capture);
}

#[test]
#[should_panic(expected = "refuses an unknown major version")]
fn a_capture_from_a_future_schema_is_refused_not_guessed_at() {
    let serial = fixture("aarch64-selftest-complete.txt").replace("v=1 ", "v=2 ");
    let capture = decode_capture(&serial);
    assert_capture_contract("future-version", &capture);
}

#[test]
#[should_panic(expected = "truncated capture")]
fn a_begin_with_no_end_is_refused() {
    let serial: String = fixture("aarch64-selftest-complete.txt")
        .lines()
        .filter(|line| !line.starts_with("[BXCAP:END"))
        .collect::<Vec<_>>()
        .join("\n");
    let capture = decode_capture(&serial);
    assert_capture_contract("no-end", &capture);
}

#[test]
#[should_panic(expected = "section CNT is missing")]
fn a_complete_capture_missing_a_section_is_caught() {
    let serial: String = fixture("aarch64-selftest-complete.txt")
        .lines()
        .filter(|line| !line.starts_with("[BXCAP:CNT"))
        .collect::<Vec<_>>()
        .join("\n");
    // `records=` is recomputed so the section check, not the count check, is
    // the assertion under test.
    let capture = decode_capture(&serial);
    let declared = capture.records.len();
    let serial = serial.replace(
        &format!("records={}", capture.end.u64("records")),
        &format!("records={declared}"),
    );
    let capture = decode_capture(&serial);
    assert_capture_contract("missing-cnt", &capture);
    assert_untruncated_sections("missing-cnt", &capture);
}

#[test]
#[should_panic(expected = "well-formed records decode")]
fn a_records_count_that_disagrees_with_the_wire_is_caught() {
    let serial = fixture("aarch64-selftest-complete.txt").replace("records=76", "records=75");
    let capture = decode_capture(&serial);
    assert_capture_contract("bad-count", &capture);
}

#[test]
#[should_panic(expected = "the refusal is not stated")]
fn dropping_the_refusal_note_is_caught() {
    let serial: String = fixture("aarch64-selftest-sched-lock-held.txt")
        .lines()
        .filter(|line| !line.starts_with("[BXCAP:NOTE"))
        .collect::<Vec<_>>()
        .join("\n");
    let capture = decode_capture(&serial);
    let declared = capture.records.len();
    let serial = serial.replace("records=69", &format!("records={declared}"));
    let capture = decode_capture(&serial);
    assert_capture_contract("no-note", &capture);
}

#[test]
#[should_panic(expected = "disagrees with its own accounting")]
fn a_verdict_that_contradicts_its_accounting_is_caught() {
    let serial =
        fixture("aarch64-selftest-sched-lock-held.txt").replace("verdict=partial", "verdict=complete");
    let capture = decode_capture(&serial);
    assert_capture_contract("bad-verdict", &capture);
}

#[test]
#[should_panic(expected = "over the emitter's stated bound")]
fn a_capture_that_overran_the_budget_is_caught() {
    let serial = fixture("aarch64-selftest-tiny-budget.txt").replace("bytes=616", "bytes=61600");
    let capture = decode_capture(&serial);
    assert_byte_bound("overrun", &capture, true);
}
