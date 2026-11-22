use bcc::pp::{self, Kind, Location, Token};
use std::path::Path;
use std::process::Command;

#[test]
fn n1570_6_10_3_5_example_3() {
    let cconf = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/cconf");
    let result = Command::new("python3")
        .args([
            "-B",
            "-c",
            r#"
import sys
from types import SimpleNamespace
import run

case, = (case for case in run.load_manifest(run.HERE / "cases.json")
         if case["id"] == "macro-scope-example3" and case["mode"] == "E")
reason = run.run_case(case, SimpleNamespace(cc=sys.argv[1], cc_arg=[], mode="E"))
if reason is not None:
    sys.exit(reason)
"#,
            env!("CARGO_BIN_EXE_bcc"),
        ])
        .current_dir(cconf)
        .output()
        .expect("run cconf's mode E comparison");
    assert!(
        result.status.success(),
        "macro-scope-example3: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

fn location((line, column): (usize, usize)) -> Location {
    Location { line, column }
}

#[test]
fn splice_inside_token() {
    // N1570 5.1.1.2p1 and 6.4p4. Physical ranges match the E25 check in 5797bde6.
    let input = b"SPLI\\\r\nCED 12??/\r\n34/* gap\r\n*/+\\\r\n+ end\r\n";
    let expected = [
        ("SPLICED", Kind::Identifier, false, (1, 1), (2, 4)),
        ("1234", Kind::Number, true, (2, 5), (3, 3)),
        ("++", Kind::Punctuator, true, (4, 3), (5, 2)),
        ("end", Kind::Identifier, true, (5, 3), (5, 6)),
        ("\n", Kind::Newline, false, (5, 6), (6, 1)),
    ]
    .map(|(spelling, kind, space_before, begin, end)| Token {
        spelling,
        kind,
        space_before,
        begin: location(begin),
        end: location(end),
    });

    let source = pp::scan(input).expect("scan valid preprocessing tokens");
    assert_eq!(source.tokens().collect::<Vec<_>>(), expected);
}

#[test]
fn pp_phases_is_error_not_panic() {
    let malformed = [
        (b"one\\\r\n two /*/".as_slice(), (2, 6), "unterminated comment"),
        (b"??= L\"abc\\", (1, 6), "unterminated string literal"),
        (b"'\\x'\n", (1, 2), "invalid escape sequence"),
        (b"\r\n\xf0\x9f", (2, 1), "invalid UTF-8 in source"),
        (b"abc\0\n", (1, 4), "null byte in source"),
    ];
    for (input, position, message) in malformed {
        let error = pp::scan(input).expect_err("malformed source must return a diagnostic");
        assert_eq!(error.location, location(position), "{input:?}");
        assert_eq!(error.message, message, "{input:?}");
    }
}
