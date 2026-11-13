//! Report host filesystem format evidence staged on the disk.
use breenix_suite::suite::{case, category, fail, running_case, suite, CaseResult, Suite};

static SUITE: Suite = suite("filesystem", "Copy-on-write filesystem", &[
    category("format", "Formatter images pass the independent checker and violating images are refused", &[
        case("mkfs-empty-clean", "Empty formatted images pass the full checker", host_case),
        case("mkfs-populate-clean", "Populated formatted images pass the full checker", host_case),
        case("reproducible-image", "Fixed inputs produce byte-identical images", host_case),
        case("violations-refused", "Images violating format invariants are refused", host_case),
    ]),
]);

fn host_case() -> CaseResult {
    let id = running_case().ok_or("the case could not tell which case it is")?;
    let path = format!("/usr/local/filesystem-suite/host/{id}");
    let text = std::fs::read_to_string(&path).map_err(|_| format!("the host harness left no record {path}"))?;
    match text.lines().next().unwrap_or("") {
        "PASS" => Ok(()),
        line if line.starts_with("FAIL ") => fail(format!("host: {}", &line[5..])),
        line => fail(format!("{path} reads {line:?}, not PASS or FAIL")),
    }
}

fn main() { SUITE.run() }
