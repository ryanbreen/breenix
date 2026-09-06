//! The `BXCAP v1` oracle: a minimal decoder, run over committed self-test
//! serials.
//!
//! This is PR-3's red-to-green leg in the form the plan asks for. `main`
//! emits no `[BXCAP:` bytes, so the assertions below are red there for the
//! trivial reason that there is no capture to decode; what makes the suite
//! worth keeping afterwards is that it is a real decoder, and it fails on a
//! capture that is malformed as well as on one that is missing.
//!
//! # What it decodes, and what that pins
//!
//! * `BEGIN`/`END` bracketing. A `BEGIN` with no `END` is the definition of
//!   a truncated capture; this suite rejects it rather than scoring the
//!   fragment.
//! * That a record's own bytes are intact. A record may be PRECEDED on its
//!   line by another writer's output -- on x86 the scheduler's raw `[SW]<K>`
//!   markers share the port with no newline of their own -- but bytes
//!   spliced INSIDE the record make it undecodable, which is the #847
//!   interleaving shape.
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
//! 8 `#[should_panic]` legs damage a fixture in memory -- strip the version,
//! bump it to an unknown one, drop the `END`, delete a section, decrement
//! `records=`, drop the refusal note, contradict the verdict, overstate
//! `bytes=` -- and assert the decoder rejects each. Without those, a decoder
//! that returned early would read as green.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

const SERIAL_DIR: &str = "docs/planning/green-program/failure-capture/serials/pr3";
/// PR-4's fixtures: the terminal edges. Kept in their own directory rather
/// than mixed into PR-3's, so a reader can see which round produced which
/// capture and so the two rounds' anchored counts stay independent.
const SERIAL_DIR_PR4: &str = "docs/planning/green-program/failure-capture/serials/pr4";
/// PR-4's RED baselines: real boots that reach the same terminal edge and
/// emit no capture at all. They are not captures and are not decoded; what
/// they are checked for is the absence the green fixtures are measured
/// against.
const SERIAL_DIR_PR4_RED: &str =
    "docs/planning/green-program/failure-capture/serials/pr4-red";
const RECORD_SOURCE: &str = "kernel/src/capture/record.rs";

/// The only schema major version this decoder understands.
const KNOWN_VERSION: u64 = 1;

/// The `sections_skipped` bit each section token owns, from
/// `kernel/src/capture/sections.rs`'s `SECTION_*` constants. Each row is
/// cross-checked against that source below, so the two cannot drift apart
/// silently.
const SECTION_BITS: [(&str, u32); 6] = [
    ("EDGE", 0),
    ("CPU", 1),
    ("EV", 2),
    ("CNT", 3),
    ("RING", 4),
    ("THR", 5),
];

/// `sections_skipped` bit for `THR`.
const THR_BIT: u64 = 1 << 5;

/// The bit a section token owns, for a token that names a section;
/// `BEGIN`, `END` and `NOTE` are not sections and own no bit.
/// claim-lint:ok: the flagged word is Rust's `Option::None` in the return
/// type below, not a claim; the token-to-bit mapping it returns is checked
/// against kernel/src/capture/sections.rs by
/// the_section_bit_table_matches_the_emitters_own_numbering.
fn section_bit(token: &str) -> Option<u64> {
    SECTION_BITS
        .iter()
        .find(|(name, _)| *name == token)
        .map(|(_, bit)| 1u64 << bit)
}

/// The token of a line the decoder refused: the word after `[BXCAP:`, up to
/// the first space. A fragment the byte budget cut keeps its token, because
/// `Writer::open()` writes the token before any of the record's content.
fn fragment_token(fragment: &str) -> String {
    let start = fragment
        .find("[BXCAP:")
        .unwrap_or_else(|| panic!("not a BXCAP fragment: {fragment}"));
    fragment[start + "[BXCAP:".len()..]
        .split(|c: char| c == ' ' || c == ']' || c == '\r')
        .next()
        .unwrap_or("")
        .to_string()
}

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

/// Decode one line into a record. A line that is not a well-formed
/// `[BXCAP:...]` record yields no record.
///
/// "Well-formed" is deliberately strict: the line must start with `[BXCAP:`
/// and end with `]`. A record the emitter's byte budget cut mid-line has no
/// closing `]`, so it is not decoded and not counted -- which is exactly the
/// rule `records=` is written against.
fn decode_line(line: &str) -> Option<Record> {
    let line = line.trim_end_matches(['\r', '\n']);
    // A record may be PRECEDED on its line by bytes from another writer on
    // the same UART. On x86 that is routine: the scheduler emits raw
    // single-character `[SW]<K>` markers to the same port with no newline of
    // their own, and `docker/qemu/run-x86-prod-profile-boot-test.sh` already
    // requires its own marker matches to be substring matches for that
    // reason. A leading prefix does not damage the record.
    //
    // Bytes spliced INSIDE the record do damage it, and are not accepted:
    // the inner text must contain no further `[` or `]`. That is the #847
    // interleaving shape, and rejecting it here is what makes the fixture
    // check meaningful.
    let start = line.find("[BXCAP:")?;
    let rest = &line[start + "[BXCAP:".len()..];
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
    /// The well-formed records from `BEGIN` up to but NOT including `END`.
    records: Vec<Record>,
    /// Lines between the brackets that did not decode. At most one, and only
    /// when the byte budget cut a record.
    undecodable: Vec<String>,
}

/// A `[BXCAP:` line in `serial` must be CRLF-terminated.
///
/// The schema names `\r\n` as the record terminator and the emitter writes
/// it, so the fixture has to show it. `.gitattributes` marks these files
/// `-text` for exactly this reason: a CRLF normalisation would delete the
/// byte this assertion is about and leave the suite pinning a property its
/// own fixture no longer demonstrates.
fn assert_records_are_crlf_terminated(name: &str, serial: &str) {
    let mut checked = 0;
    for (i, line) in serial.split('\n').enumerate() {
        if !line.contains("[BXCAP:") {
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
        .filter(|(_, line)| line.contains("[BXCAP:BEGIN"))
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
        .find(|(_, line)| line.contains("[BXCAP:END"))
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

/// The contract a capture must satisfy, whatever its outcome.
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

    // A fragment on the wire belongs to the section that was mid-record when
    // the budget ran out, and THAT section must not be claiming completion.
    // `Writer::open()` only refuses a record that starts with the budget
    // already spent; a record cut mid-write is reported by `close()`'s
    // return value alone, and a section that dropped it would clear its own
    // bit here over a fragment. `sections_skipped=` is the one field a
    // reader has for learning which section a truncation ate, so this is the
    // assertion that makes it worth reading.
    for fragment in &capture.undecodable {
        let token = fragment_token(fragment);
        let Some(bit) = section_bit(&token) else {
            // BEGIN/END/NOTE are not sections and own no bit. A cut BEGIN or
            // END is caught by the bracket check above, and a cut NOTE by
            // the THR contract below.
            assert!(
                matches!(token.as_str(), "BEGIN" | "END" | "NOTE"),
                "{name}: fragment carries the unknown token `{token}`: {fragment}"
            );
            continue;
        };
        assert!(
            skipped & bit != 0,
            "{name}: the byte budget cut a `{token}` record ({fragment:?}) but \
             sections_skipped={skipped:#x} does not carry {token}'s bit {bit:#x} -- \
             the capture is claiming a fragment as a completed section"
        );
    }
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
        // A truncated capture stopped before THR was reached, and
        // `truncated=1` with THR's bit set is that explanation. Demanding
        // the refusal note here would demand a section the budget stopped
        // before.
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

/// The cfg attribute guarding each `BXCAP_BUDGET_BYTES` arm, spelled exactly
/// as `record.rs` spells it.
///
/// Matched on the WHOLE cfg attribute, not on a feature name: an arm's
/// attribute can contain another arm's feature name, and a substring match on
/// the name alone silently returns one arm's budget for another, leaving the
/// bound check many times too loose. Found by running this PR's own budget
/// mutation on a real boot: the mutated capture overran the exact bound and
/// this helper passed it anyway.
const ARM_ORDINARY: &str = "#[cfg(not(feature = \"capture_selftest_budget_mutation\"))]";
const ARM_TINY: &str = "#[cfg(feature = \"capture_selftest_tiny_budget\")]";
const ARM_CUT_IN_RECORD: &str = "#[cfg(feature = \"capture_selftest_cut_in_record\")]";
const ARM_CUT_AT_TERMINATOR: &str = "#[cfg(feature = \"capture_selftest_cut_at_terminator\")]";

/// Whitespace runs collapsed to one space, so an attribute a formatter wrapped
/// across lines still matches the single-line spelling above.
fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `BXCAP_BUDGET_BYTES` for one cfg arm, read out of the emitter's own source
/// so this suite cannot pin a number the kernel no longer uses.
fn budget_bytes(arm: &str) -> u64 {
    let source = collapse_whitespace(&read(RECORD_SOURCE));
    let needle = format!(
        "{} pub const BXCAP_BUDGET_BYTES: u32 = ",
        collapse_whitespace(arm)
    );
    let at = source
        .find(&needle)
        .unwrap_or_else(|| panic!("no BXCAP_BUDGET_BYTES guarded by `{arm}` in {RECORD_SOURCE}"));
    let rest = &source[at + needle.len()..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits
        .parse::<u64>()
        .unwrap_or_else(|_| panic!("could not parse BXCAP_BUDGET_BYTES for `{arm}`: {rest:.40}"))
}

/// The emitter's stated bound: section content is capped by the budget, and
/// the two bracket lines plus one line terminator sit outside it.
fn assert_byte_bound(name: &str, capture: &Capture, arm: &str) {
    let budget = budget_bytes(arm);
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

fn fixture_pr4(name: &str) -> String {
    read(&format!("{SERIAL_DIR_PR4}/{name}"))
}

#[test]
fn every_budget_arm_is_read_separately() {
    // Anti-vacuity for `budget_bytes`: if two arms resolved to the same
    // constant, one fixture's bound check would be checking another arm's
    // budget and would pass on a capture far over its own limit.
    let ordinary = budget_bytes(ARM_ORDINARY);
    let arms = [
        ("tiny", budget_bytes(ARM_TINY)),
        ("cut-in-record", budget_bytes(ARM_CUT_IN_RECORD)),
        ("cut-at-terminator", budget_bytes(ARM_CUT_AT_TERMINATOR)),
    ];
    for (name, value) in arms {
        assert!(
            value < ordinary,
            "the {name} budget arm must be smaller than the ordinary one (read \
             {value}, ordinary {ordinary}); equal values mean the cfg arms are not \
             being told apart"
        );
    }
    for (i, (left_name, left)) in arms.iter().enumerate() {
        for (right_name, right) in &arms[i + 1..] {
            assert_ne!(
                left, right,
                "the {left_name} and {right_name} budget arms read as the same number \
                 ({left}); each one exists to stop the writer somewhere different"
            );
        }
    }
}

#[test]
fn the_section_bit_table_matches_the_emitters_own_numbering() {
    let sections = read("kernel/src/capture/sections.rs");
    for (token, bit) in SECTION_BITS {
        let decl = format!("pub const SECTION_{token}: u32 = {bit};");
        assert!(
            sections.contains(&decl),
            "sections.rs no longer declares `{decl}`; this suite reads \
             sections_skipped= through that numbering and would now be checking the \
             wrong bit"
        );
    }
    // The bitmap covers exactly these, so a section added without a row here
    // would be invisible to the fragment cross-check.
    let declared = sections.matches("pub const SECTION_").count();
    assert_eq!(
        declared,
        SECTION_BITS.len(),
        "sections.rs declares {declared} SECTION_* constants and this suite knows \
         {}; a section the table does not carry is a section the fragment check \
         cannot police",
        SECTION_BITS.len()
    );
    assert!(
        sections.contains(&format!("pub const SECTIONS_ALL: u64 = (1 << {}) - 1;", SECTION_BITS.len())),
        "SECTIONS_ALL must cover exactly the {} sections this suite knows about",
        SECTION_BITS.len()
    );
}

#[test]
fn selftest_capture_with_the_scheduler_read_granted_is_complete() {
    let serial = fixture("aarch64-selftest-complete.txt");
    assert_records_are_crlf_terminated("complete", &serial);
    let capture = decode_capture(&serial);
    assert_capture_contract("complete", &capture);
    assert_untruncated_sections("complete", &capture);
    assert_byte_bound("complete", &capture, ARM_ORDINARY);

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
    // The point of the section order: a refused THR costs THR alone, and
    // leaves the sections above it intact.
    assert_untruncated_sections("sched-lock-held", &capture);
    assert_byte_bound("sched-lock-held", &capture, ARM_ORDINARY);

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
    assert_byte_bound("tiny-budget", &capture, ARM_TINY);

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

/// The union of the six section bits: what `sections_skipped=` reads from a
/// capture that completed no section at its own full width.
/// claim-lint:ok: the count is the length of SECTION_BITS, which
/// the_section_bit_table_matches_the_emitters_own_numbering checks against
/// sections.rs's own SECTION_* census.
fn all_section_bits() -> u64 {
    SECTION_BITS.iter().map(|(_, bit)| 1u64 << bit).sum()
}

#[test]
fn a_section_whose_own_record_the_budget_cut_does_not_claim_completion() {
    let serial = fixture("aarch64-selftest-cut-in-record.txt");
    assert_records_are_crlf_terminated("cut-in-record", &serial);
    let capture = decode_capture(&serial);
    assert_capture_contract("cut-in-record", &capture);
    assert_byte_bound("cut-in-record", &capture, ARM_CUT_IN_RECORD);

    assert_eq!(capture.end.u64("truncated"), 1);
    assert_eq!(capture.end.text("verdict"), "partial");
    // The budget ends inside `EDGE`, the first budgeted record. `EDGE` emits
    // one record and then returns: no later `open()` refusal can speak for
    // it, so its bit is set here only because the section carried
    // `Writer::close()`'s verdict back out. This is the leg the emitter used
    // to fail -- it reported `EDGE` complete over the fragment below.
    assert_eq!(
        capture.undecodable.len(),
        1,
        "expected the one cut EDGE record, got {:?}",
        capture.undecodable
    );
    assert_eq!(fragment_token(&capture.undecodable[0]), "EDGE");
    assert_eq!(
        capture.end.u64("sections_skipped"),
        all_section_bits(),
        "the budget ran out in the first section, so no section completed"
    );
    // `records=1` is the BEGIN line alone: the EDGE fragment has no `]`.
    assert_eq!(capture.end.u64("records"), 1);
}

#[test]
fn a_record_whose_terminator_the_budget_cut_is_still_counted() {
    let serial = fixture("aarch64-selftest-cut-at-terminator.txt");
    assert_records_are_crlf_terminated("cut-at-terminator", &serial);
    let capture = decode_capture(&serial);
    assert_capture_contract("cut-at-terminator", &capture);
    assert_byte_bound("cut-at-terminator", &capture, ARM_CUT_AT_TERMINATOR);

    assert_eq!(capture.end.u64("truncated"), 1);
    assert_eq!(capture.end.text("verdict"), "partial");
    // The one boundary where "the budget cut this record" and "a reader can
    // parse this record" disagree: `EDGE`'s `]` landed and its CRLF did not,
    // `close_dangling_record()` supplied the terminator with the budget
    // suspended, and the line decodes. So no fragment is left behind,
    // `records=` counts BEGIN and EDGE, and EDGE does NOT appear in
    // sections_skipped.
    assert!(
        capture.undecodable.is_empty(),
        "the record kept its `]`, so nothing should have failed to decode: {:?}",
        capture.undecodable
    );
    assert_eq!(capture.end.u64("records"), 2);
    assert!(capture.records.iter().any(|r| r.token == "EDGE"));
    assert_eq!(
        capture.end.u64("sections_skipped"),
        all_section_bits() & !section_bit("EDGE").expect("EDGE is a section"),
        "EDGE's record is on the wire whole, so EDGE alone completed"
    );
}

/// Decode each `.txt` fixture in one directory, returning how many.
fn decode_every_fixture_in(dir_rel: &str) -> usize {
    let dir = repo_path(dir_rel);
    let mut checked = 0;
    for entry in fs::read_dir(&dir).unwrap_or_else(|_| panic!("{dir_rel} must exist")) {
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
    checked
}

#[test]
fn every_committed_fixture_decodes() {
    // Census-anchored, not a literal list: a fixture added to either
    // directory is decoded the moment it lands, and an empty directory is a
    // failure rather than a silent pass.
    let pr3 = decode_every_fixture_in(SERIAL_DIR);
    assert!(
        pr3 >= 6,
        "expected at least the six PR-3 fixtures under {SERIAL_DIR}, decoded {pr3}"
    );
    let pr4 = decode_every_fixture_in(SERIAL_DIR_PR4);
    assert!(
        pr4 >= 3,
        "expected at least the three aarch64 PR-4 terminal-edge fixtures under \
         {SERIAL_DIR_PR4}, decoded {pr4}"
    );
}

#[test]
fn the_red_baselines_carry_no_capture_at_all() {
    // The other half of PR-4's red-to-green leg, and the half a decoder
    // cannot check: these are real boots that reached the same terminal edge
    // and emitted no capture. A single `[BXCAP:` byte in one of them would
    // mean the baseline is not a baseline.
    let dir = repo_path(SERIAL_DIR_PR4_RED);
    let mut checked = 0;
    for entry in fs::read_dir(&dir).expect("serials/pr4-red must exist") {
        let path = entry.expect("readable dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("txt") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let serial = fs::read_to_string(&path).expect("readable baseline");
        let hits = serial.matches("[BXCAP:").count();
        assert_eq!(
            hits, 0,
            "{name}: a RED baseline carries {hits} [BXCAP: occurrences. These files \
             exist to show the terminal edge emitting nothing; one that emits \
             something is evidence for the opposite claim."
        );
        // And each one must actually have REACHED a terminal edge, or it is
        // a baseline for no edge at all.
        assert!(
            serial.contains("KERNEL PANIC") || serial.contains("[FATAL_POSTMORTEM]"),
            "{name}: no terminal edge in this baseline -- neither a panic banner nor a \
             fatal postmortem. An ordinary boot emits no capture either, and proves \
             nothing."
        );
        checked += 1;
    }
    assert!(
        checked >= 2,
        "expected at least the two aarch64 red baselines under {SERIAL_DIR_PR4_RED}, \
         checked {checked}"
    );
}

/// The contract PR-4's own oracle is written against: a complete bracketed
/// record for the named edge, with a real event tail behind it.
///
/// `verdict=` is deliberately NOT part of this: the `THR` section asks the
/// scheduler for a non-blocking read and a terminal edge taken in hard-IRQ
/// context loses that race often (measured: 1 of 4 aarch64 panic boots this
/// round came back `complete`). A capture that reports the refusal honestly
/// is a working capture, and demanding `verdict=complete` would be demanding
/// the race go one way.
fn assert_terminal_edge_capture(name: &str, capture: &Capture, edge: &str) {
    assert_capture_contract(name, capture);
    assert_eq!(
        capture.begin.text("edge"),
        edge,
        "{name}: this fixture is supposed to be the {edge} edge"
    );
    assert_eq!(
        capture.end.u64("truncated"),
        0,
        "{name}: the capture was cut off by its own byte budget"
    );
    let events = capture.records.iter().filter(|r| r.token == "EV").count();
    assert!(
        events > 0,
        "{name}: the EV section is empty. A terminal-edge capture with no event tail \
         carries no pre-failure timeline, which is the thing it exists for."
    );
    let cpu_rows = capture.records.iter().filter(|r| r.token == "CPU").count();
    assert_eq!(
        cpu_rows, 1,
        "{name}: expected exactly one [BXCAP:CPU] row -- the capturing CPU's own. A \
         peer CPU's per-CPU block is reachable only through its own base register, so \
         a row for it would be a guess; the scheduler-side view of the other CPUs is \
         what THR carries."
    );
    assert!(
        capture.records.iter().any(|r| r.token == "EDGE"),
        "{name}: no EDGE record, so nothing says what fired this capture"
    );
}

#[test]
fn the_aarch64_panic_handler_capture_is_complete() {
    let serial = fixture_pr4("aarch64-panic-complete.txt");
    assert_records_are_crlf_terminated("panic-complete", &serial);
    let capture = decode_capture(&serial);
    assert_terminal_edge_capture("panic-complete", &capture, "PANIC");
    assert_untruncated_sections("panic-complete", &capture);
    assert_eq!(capture.begin.text("arch"), "aarch64");
    assert_eq!(capture.end.text("verdict"), "complete");
    // The record has to be BELOW the panic banner: that ordering is what
    // makes the message and the state read as one block, and it is pinned
    // from the source side by tests/terminal_edge_capture_structure.rs.
    let banner = serial
        .find("KERNEL PANIC")
        .expect("the fixture must carry the panic banner");
    let record = serial
        .find("[BXCAP:BEGIN")
        .expect("the fixture must carry the capture");
    assert!(
        banner < record,
        "the capture must follow the panic banner on the wire"
    );
}

#[test]
fn the_aarch64_panic_capture_states_a_refused_scheduler_read() {
    // The same edge, the other side of the THR race. This is not a defect:
    // it is the capture reporting what it could not read.
    let serial = fixture_pr4("aarch64-panic-sched-lock-held.txt");
    let capture = decode_capture(&serial);
    assert_terminal_edge_capture("panic-refused", &capture, "PANIC");
    assert_eq!(capture.end.text("verdict"), "partial");
    assert!(
        capture
            .records
            .iter()
            .any(|r| r.token == "NOTE" && r.body.contains("sched_lock_held")),
        "the refused scheduler read must be stated, not merely absent"
    );
}

#[test]
fn the_x86_panic_handler_capture_is_complete_and_precedes_exit_qemu() {
    let serial = fixture_pr4("x86_64-panic-complete.txt");
    assert_records_are_crlf_terminated("x86-panic", &serial);
    let capture = decode_capture(&serial);
    assert_terminal_edge_capture("x86-panic", &capture, "PANIC");
    assert_untruncated_sections("x86-panic", &capture);
    assert_eq!(capture.begin.text("arch"), "x86_64");
    assert_eq!(capture.end.text("verdict"), "complete");
    // The ordering claim, measured on the wire: `exit_qemu(Failed)` ends the
    // QEMU process, so a capture emitted after it would not exist. The whole
    // record is here, END included, which is only possible if it ran first.
    let banner = serial
        .find("KERNEL PANIC")
        .expect("the fixture must carry the panic banner");
    let record = serial
        .find("[BXCAP:BEGIN")
        .expect("the fixture must carry the capture");
    let end = serial
        .find("[BXCAP:END")
        .expect("a capture cut short by exit_qemu would have no END");
    assert!(
        banner < record && record < end,
        "expected banner -> BEGIN -> END, got offsets {banner} / {record} / {end}"
    );
    // -smp 1: no peer CPU to hold the scheduler lock, so THR is emitted.
    assert!(
        capture.records.iter().any(|r| r.token == "THR"),
        "the x86 gate is -smp 1, where the non-blocking scheduler read has no peer \
         CPU to lose to"
    );
}

#[test]
fn the_aarch64_fatal_postmortem_capture_is_complete_and_keeps_the_wide_dump() {
    let serial = fixture_pr4("aarch64-fault-postmortem-complete.txt");
    assert_records_are_crlf_terminated("fault", &serial);
    let capture = decode_capture(&serial);
    assert_terminal_edge_capture("fault", &capture, "FAULT");
    assert_untruncated_sections("fault", &capture);
    assert_eq!(capture.begin.text("arch"), "aarch64");

    // The capture is section 7's first act, and the unbounded per-CPU ring
    // dump still follows it. PR-4 adds the bounded record beside the wide
    // one rather than replacing it; this is that claim measured on the wire
    // rather than read off the source.
    let postmortem = serial
        .find("[FATAL_POSTMORTEM]")
        .expect("the fixture must carry the postmortem banner");
    let record = serial
        .find("[BXCAP:BEGIN")
        .expect("the fixture must carry the capture");
    let wide = serial
        .find("[TRACE] ====== TRACE BUFFER DUMP ======")
        .expect("section 7 must still emit the unbounded per-CPU ring dump");
    assert!(
        postmortem < record && record < wide,
        "expected postmortem banner -> [BXCAP] record -> wide ring dump, got offsets \
         {postmortem} / {record} / {wide}"
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
#[should_panic(expected = "well-formed records decode")]
fn a_record_with_another_writers_bytes_spliced_into_it_is_not_decoded() {
    // The #847 shape, reproduced against a fixture: a peer CPU's line lands
    // inside a record rather than before it. The record stops decoding, so
    // the count no longer matches what END declares.
    let serial = fixture("aarch64-selftest-complete.txt").replacen(
        "[BXCAP:CNT ",
        "[BXCAP:C[TEST:process:some_test:PASS]NT ",
        1,
    );
    let capture = decode_capture(&serial);
    assert_capture_contract("spliced", &capture);
}

#[test]
fn a_leading_prefix_from_another_writer_does_not_damage_a_record() {
    // The x86 case, and the reason `decode_line` searches for `[BXCAP:`
    // rather than requiring it at column 0: the scheduler's raw `[SW]<K>`
    // markers share the UART with no newline of their own. The committed x86
    // fixture carries exactly this on its BEGIN line.
    let serial = fixture("x86_64-selftest-complete.txt");
    let capture = decode_capture(&serial);
    assert_capture_contract("x86-prefixed", &capture);
    assert_eq!(capture.begin.text("arch"), "x86_64");
    assert_eq!(capture.end.text("verdict"), "complete");
    assert_untruncated_sections("x86-prefixed", &capture);
    // -smp 1: there is no peer CPU to be holding the scheduler lock, so THR
    // is emitted rather than refused.
    assert!(
        capture.records.iter().any(|r| r.token == "THR"),
        "the x86 fixture is -smp 1, where the non-blocking scheduler read has \
         no peer CPU to lose to"
    );
}

#[test]
#[should_panic(expected = "claiming a fragment as a completed section")]
fn a_section_that_claims_completion_over_its_own_fragment_is_caught() {
    // The pre-fix shape, applied to the fixture that exercises it: the
    // budget cut `EDGE` mid-record and the capture nevertheless clears
    // EDGE's bit. `verdict=partial` still holds, and `records=` still
    // matches the wire, so the rest of the contract does not notice -- which
    // is why this assertion has to exist separately.
    let all = all_section_bits();
    let cleared = all & !section_bit("EDGE").expect("EDGE is a section");
    let serial = fixture("aarch64-selftest-cut-in-record.txt").replace(
        &format!("sections_skipped={all:#x}"),
        &format!("sections_skipped={cleared:#x}"),
    );
    let capture = decode_capture(&serial);
    assert_capture_contract("fragment-claimed", &capture);
}

#[test]
#[should_panic(expected = "over the emitter's stated bound")]
fn a_capture_that_overran_the_budget_is_caught() {
    let serial = fixture("aarch64-selftest-tiny-budget.txt").replace("bytes=616", "bytes=61600");
    let capture = decode_capture(&serial);
    assert_byte_bound("overrun", &capture, ARM_TINY);
}

// ---------------------------------------------------------------------------
// Failure-capture PR-7: the `edge=LOCKUP` fixtures.
//
// These are the one non-terminal edge, and the only fixtures in this suite
// produced by a construction rather than by an injected fault: a CPU1-pinned
// kernel thread holds the REAL scheduler guard under masked interrupts for
// longer than the detector's own threshold while CPU0 keeps taking timer
// interrupts. So `[BXCAP:NOTE sched_lock_held]` here is not a lost race -- the
// refusal is the state the run deliberately established, and the receipts
// beside the fixture say which CPU held the guard and for how many ticks.
//
// Two captures in ONE boot is the other thing these fixtures carry, and it is
// what a single-episode run cannot show: `WATCHDOG_REPORTED` clearing on
// observed progress, `IN_CAPTURE` clearing on normal completion, and `seq`
// advancing.
// ---------------------------------------------------------------------------

const SERIAL_DIR_PR7: &str = "docs/planning/green-program/failure-capture/serials/pr7";
const SERIAL_DIR_PR7_RED: &str =
    "docs/planning/green-program/failure-capture/serials/pr7-red";

fn fixture_pr7(name: &str) -> String {
    read(&format!("{SERIAL_DIR_PR7}/{name}"))
}

/// Split a serial carrying several captures into one text per capture.
///
/// `decode_capture` refuses a serial with more than one `BEGIN` on purpose --
/// scoring two interleaved captures as one is exactly the confusion the `seq`
/// field exists to prevent. So a multi-episode run is split FIRST, on the
/// bracket lines, and each block then goes through the unchanged decoder
/// contract. A `BEGIN` with no following `END` keeps everything after it, so
/// the decoder still sees the truncation rather than this splitter hiding it.
fn split_captures(serial: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current: Option<Vec<String>> = None;
    for line in serial.split('\n') {
        if line.contains("[BXCAP:BEGIN") {
            current = Some(Vec::new());
        }
        let Some(block) = current.as_mut() else {
            continue;
        };
        block.push(line.to_string());
        if line.contains("[BXCAP:END") {
            blocks.push(block.join("\n"));
            current = None;
        }
    }
    if let Some(block) = current {
        blocks.push(block.join("\n"));
    }
    blocks
}

/// The `edge=LOCKUP` contract, applied to one capture.
///
/// `verdict=partial` and `sections_skipped=0x20` are REQUIRED here, unlike the
/// terminal-edge contract above which deliberately leaves the verdict open.
/// The difference is what produced the fixture: a terminal edge races the
/// scheduler read and can win, while this run establishes the hold first and
/// then waits past the detector's threshold, so a `complete` verdict would mean
/// the guard was not actually held and the run showed nothing.
/// claim-lint:ok: 2 of 2 episodes came back verdict=partial sections_skipped=0x20
/// in docs/planning/green-program/failure-capture/serials/pr7/aarch64-lockup-two-episodes.txt
fn assert_lockup_capture(name: &str, capture: &Capture) {
    assert_capture_contract(name, capture);
    assert_untruncated_sections(name, capture);
    assert_byte_bound(name, capture, ARM_ORDINARY);

    assert_eq!(capture.begin.text("edge"), "LOCKUP");
    assert_eq!(capture.end.text("edge"), "LOCKUP");
    assert_eq!(capture.begin.text("arch"), "aarch64");
    assert_eq!(capture.begin.u64("cpu"), 0, "only CPU0 runs the detector");
    assert!(capture.begin.u64("tsfreq") > 0);
    assert!(capture.begin.u64("uptime_ms") > 0);

    // EDGE's two words: a0 = stalled CPU0-local ticks, a1 = ticks per second.
    // Read as numbers, not matched against a literal -- the exact stall count
    // depends on when in the tick the detector fired.
    let edge: Vec<&Record> = capture.records.iter().filter(|r| r.token == "EDGE").collect();
    assert_eq!(edge.len(), 1, "{name}: one EDGE record");
    assert_eq!(edge[0].text("kind"), "LOCKUP");
    let a1 = edge[0].u64("a1");
    assert_eq!(a1, 1000, "{name}: a1 is TARGET_TIMER_HZ");
    let a0 = edge[0].u64("a0");
    assert!(
        a0 >= 5000,
        "{name}: a0={a0} is below the detector's own LOCKUP_THRESHOLD_TICKS"
    );

    let cpu: Vec<&Record> = capture.records.iter().filter(|r| r.token == "CPU").collect();
    assert_eq!(cpu.len(), 1, "{name}: one CPU row");
    assert_eq!(cpu[0].u64("cpu"), 0);
    assert_eq!(cpu[0].u64("hardirq"), 1, "{name}: the dump runs in hard IRQ");
    assert_eq!(cpu[0].text("q"), "exact");
    let cpu_fields = ["preempt", "nr", "softirq", "uptime_ms"];
    for field in cpu_fields {
        assert!(cpu[0].fields.contains_key(field), "{name}: CPU row lost {field}");
    }

    let events: Vec<&Record> = capture.records.iter().filter(|r| r.token == "EV").collect();
    assert!(!events.is_empty(), "{name}: no EV rows");
    let ev_fields = ["cpu", "i", "ts", "type", "n", "p", "f"];
    for field in ev_fields {
        assert!(events[0].fields.contains_key(field), "{name}: EV row lost {field}");
    }
    let counters: Vec<&Record> = capture.records.iter().filter(|r| r.token == "CNT").collect();
    assert!(!counters.is_empty(), "{name}: no CNT rows");

    let ring: Vec<&Record> = capture.records.iter().filter(|r| r.token == "RING").collect();
    assert_eq!(ring.len(), 1, "{name}: one RING row");
    assert_eq!(ring[0].u64("cpu"), 0);
    assert_eq!(ring[0].u64("enabled"), 1, "{name}: tracing must still be enabled");
    assert!(ring[0].u64("kept") > 0, "{name}: RING kept 0 events");
    let ring_fields = ["writes", "dropped", "span_us"];
    for field in ring_fields {
        assert!(ring[0].fields.contains_key(field), "{name}: RING row lost {field}");
    }

    // The refusal, stated, and NO fabricated rows behind it.
    assert!(
        capture
            .records
            .iter()
            .any(|r| r.token == "NOTE" && r.body.trim() == "sched_lock_held"),
        "{name}: the held guard must be reported as `[BXCAP:NOTE sched_lock_held]`"
    );
    assert!(
        !capture.records.iter().any(|r| r.token == "THR"),
        "{name}: the guard was held, so there is nothing THR could truthfully say"
    );

    // The accounting: bits 0-4 complete, bit 5 (THR) skipped, and the verdict
    // word agreeing with that rather than reading `complete` beside a refusal.
    assert_eq!(capture.end.u64("truncated"), 0);
    assert_eq!(capture.end.u64("sections_skipped"), THR_BIT);
    assert_eq!(
        (!capture.end.u64("sections_skipped")) & 0x3f,
        0x1f,
        "{name}: sections 0-4 must all be complete"
    );
    assert_eq!(capture.end.text("verdict"), "partial");
}

#[test]
fn the_lockup_edge_emits_one_capture_per_episode_in_one_boot() {
    let serial = fixture_pr7("aarch64-lockup-two-episodes.txt");
    assert_records_are_crlf_terminated("lockup-two-episodes", &serial);
    let blocks = split_captures(&serial);
    assert_eq!(
        blocks.len(),
        2,
        "the oracle ran two episodes; the serial must carry two brackets"
    );
    let first = decode_capture(&blocks[0]);
    let second = decode_capture(&blocks[1]);
    assert_lockup_capture("lockup-episode-0", &first);
    assert_lockup_capture("lockup-episode-1", &second);

    // Sequencing, which is the whole reason two episodes were run: `seq`
    // advances, and the second capture's ring shows MORE writes with tracing
    // still on. Both would be impossible if the first capture had left
    // `IN_CAPTURE` latched or the detector had left `WATCHDOG_REPORTED` set.
    let first_seq = first.begin.u64("seq");
    let second_seq = second.begin.u64("seq");
    assert!(
        second_seq > first_seq,
        "seq must advance across episodes: {first_seq} then {second_seq}"
    );
    assert_eq!(first.end.u64("seq"), first_seq);
    assert_eq!(second.end.u64("seq"), second_seq);

    let first_writes = ring_writes(&first);
    let second_writes = ring_writes(&second);
    assert!(
        second_writes > first_writes,
        "the second episode's ring must have taken more writes ({first_writes} \
         then {second_writes}); the dump no longer disables tracing"
    );
}

/// `RING writes=` for the one RING row in a capture.
fn ring_writes(capture: &Capture) -> u64 {
    let row = capture.records.iter().find(|r| r.token == "RING");
    row.expect("a RING row").u64("writes")
}

#[test]
fn the_lockup_red_baseline_carries_no_capture_at_all() {
    let dir = repo_path(SERIAL_DIR_PR7_RED);
    let mut checked = 0;
    for entry in fs::read_dir(&dir).expect("serials/pr7-red must exist") {
        let path = entry.expect("readable dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("txt") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if !name.starts_with("main-") {
            continue;
        }
        let serial = fs::read_to_string(&path).expect("readable baseline");
        let hits = serial.matches("[BXCAP:").count();
        assert_eq!(hits, 0, "{name}: a RED baseline carries {hits} BXCAP lines");
        // And it must actually have REACHED the edge, or it is a baseline for
        // no edge at all: an ordinary boot emits no capture either.
        // claim-lint:ok: 1 of 1 red baseline in this directory shows the banner
        // and the HELD line, checked by the two assertions below.
        assert!(
            serial.contains("!!! SOFT LOCKUP DETECTED !!!"),
            "{name}: no soft-lockup banner in this baseline"
        );
        assert!(
            serial.contains("Scheduler lock: HELD (possible deadlock)"),
            "{name}: the baseline must show the arm PR-7 repaired -- a refused \
             scheduler lock reported as one line"
        );
        checked += 1;
    }
    assert_eq!(checked, 1, "expected the one lockup red baseline, saw {checked}");
}

/// The first episode's block alone, for the mutation legs below.
fn lockup_episode_block() -> String {
    let serial = fixture_pr7("aarch64-lockup-two-episodes.txt");
    split_captures(&serial)
        .into_iter()
        .next()
        .expect("the fixture carries at least one capture")
}

#[test]
#[should_panic(expected = "sched_lock_held")]
fn a_lockup_capture_that_drops_the_refusal_note_is_rejected() {
    // The note's BODY is changed rather than its line deleted, so the record
    // count still agrees and the assertion this leg is about is the one that
    // fires -- not `records=`, which would reject the mutation for an
    // unrelated reason and leave the refusal check unexercised.
    let block = lockup_episode_block();
    let damaged = block.replace("NOTE sched_lock_held", "NOTE sched_lock_free");
    assert_lockup_capture("no-note", &decode_capture(&damaged));
}

#[test]
#[should_panic(expected = "truncated capture")]
fn a_lockup_capture_with_no_end_is_rejected() {
    let block = lockup_episode_block();
    let cut = block.find("[BXCAP:END").expect("the fixture has an END line");
    decode_capture(&block[..cut]);
}

#[test]
#[should_panic]
fn a_lockup_capture_whose_bitmap_contradicts_its_rows_is_rejected() {
    // The bitmap says THR completed while the NOTE says it was refused. This
    // is the shape a scorer must not accept: verdict and sections_skipped
    // have to agree with the records actually present.
    let block = lockup_episode_block();
    let damaged = block.replace("sections_skipped=0x20", "sections_skipped=0x0");
    assert_lockup_capture("contradictory-bitmap", &decode_capture(&damaged));
}

#[test]
#[should_panic(expected = "must carry two brackets")]
fn a_run_with_only_one_episode_is_rejected() {
    // Two episodes is the claim; a serial carrying one would be a boot that
    // did not re-arm, and scoring it as a pass would hide exactly that.
    // claim-lint:ok: a mutation leg -- the single-block serial is rejected here,
    // which is what keeps the two-episode assertion from being decorative.
    let serial = fixture_pr7("aarch64-lockup-two-episodes.txt");
    let single = split_captures(&serial)[0].clone();
    assert_eq!(
        split_captures(&single).len(),
        2,
        "the oracle ran two episodes; the serial must carry two brackets"
    );
}

/// The receipts are evidence too, and they are the half a serial cannot carry:
/// which CPU held the guard, whether the guard was really acquired, how long
/// the hold lasted in the kernel's own ticks, and whether any liveness counter
/// moved while it was held. A serial with two clean captures and receipts
/// saying the hold was refused would be two captures of something else.
#[test]
fn the_lockup_receipts_show_a_real_held_guard_on_a_peer_cpu() {
    let receipts = read(&format!("{SERIAL_DIR_PR7}/receipts.txt"));
    let mut rows = BTreeMap::new();
    for line in receipts.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        rows.insert(key.trim().to_string(), value.trim().to_string());
    }
    assert_eq!(rows.get("verdict").map(String::as_str), Some("DONE"));
    assert_eq!(rows.get("setup_failure").map(String::as_str), Some("0"));
    let threshold: u64 = rows["threshold_ticks"].parse().expect("threshold_ticks");
    assert!(threshold >= 5000, "threshold_ticks={threshold}");

    for episode in 0..2 {
        let key = |suffix: &str| format!("episode{episode}.{suffix}");
        assert_eq!(rows[&key("rcpt_acquired")], "1", "episode {episode} refused the guard");
        assert_eq!(rows[&key("rcpt_cpu")], "1", "episode {episode} ran off CPU1");
        assert_eq!(rows[&key("rcpt_expired")], "0", "episode {episode} hit its fail-safe");
        assert_eq!(rows[&key("arm.cpu0_deferred_requeue")], "0");
        assert_eq!(rows[&key("arm.verdict")], "ARMED");
        assert_eq!(
            rows[&key("rcpt_progress_moved_during_hold")],
            "0",
            "episode {episode}: a liveness counter moved during the hold"
        );
        let held: u64 = rows[&key("rcpt_held_ticks")].parse().expect("held ticks");
        assert!(
            held >= threshold,
            "episode {episode}: held for {held} CPU0 ticks, below the detector's \
             own threshold of {threshold}"
        );
        assert_eq!(
            rows[&key("rcpt_ctx_at_acquire")],
            rows[&key("rcpt_ctx_at_release")],
            "episode {episode}: the context-switch counter moved during the hold"
        );
        assert_eq!(
            rows[&key("rcpt_syscall_at_acquire")],
            rows[&key("rcpt_syscall_at_release")],
            "episode {episode}: the syscall counter moved during the hold"
        );
        assert_eq!(
            rows[&key("rcpt_heartbeat_at_acquire")],
            rows[&key("rcpt_heartbeat_at_release")],
            "episode {episode}: the exit-kick heartbeat moved during the hold, and \
             the detector treats that as liveness"
        );
    }
}
