use std::{fs, path::Path};

#[test]
fn six_gates_source_and_call_the_post_verdict_importer() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for name in [
        "run-aarch64-boot-test-strict.sh",
        "run-aarch64-prod-profile-boot-test.sh",
        "run-aarch64-testing-profile-boot-test.sh",
        "run-x86-boot-tests.sh",
        "run-x86-gate.sh",
        "run-x86-prod-profile-boot-test.sh",
    ] {
        let text = fs::read_to_string(root.join("docker/qemu").join(name)).unwrap();
        let lines: Vec<_> = text.lines().map(str::trim).collect();
        let code: Vec<_> = text.lines().map(str::trim)
            .filter(|line| !line.starts_with('#')).collect();
        assert!(code.iter().any(|line| line.starts_with("source ")
            && line.contains("/lib/run-inspector-import.sh")), "{name}: missing importer source");
        let expected_calls: usize = match name {
            "run-aarch64-boot-test-strict.sh" => 3,
            "run-aarch64-prod-profile-boot-test.sh" => 1,
            "run-aarch64-testing-profile-boot-test.sh" => 1,
            "run-x86-boot-tests.sh" => 2,
            "run-x86-gate.sh" => 1,
            "run-x86-prod-profile-boot-test.sh" => 2,
            _ => panic!("{name}: no expected call count on file for this script"),
        };
        let call_count = code.iter()
            .filter(|line| line.starts_with("breenix_runs_import_nonfatal "))
            .count();
        assert_eq!(call_count, expected_calls,
            "{name}: expected {expected_calls} breenix_runs_import_nonfatal call site(s), found {call_count}");

        for (offset, line) in lines.iter().enumerate()
            .filter(|(_, line)| line.starts_with("breenix_runs_import_nonfatal "))
        {
            let call_line = offset + 1;
            assert!(line.ends_with("|| :") || line.ends_with("|| true"),
                "{name}:{call_line}: import hook must discard its exit status with || : or || true");

            // Current hook calls use single-token shell references for their
            // first five arguments. Index 0 is the command; 4/5 are verdict/status.
            let arguments: Vec<_> = line.split_whitespace().collect();
            assert!(arguments.len() >= 6, "{name}:{call_line}: incomplete import arguments");
            let mut variables = 0;
            for argument in &arguments[4..=5] {
                if let Some(variable) = shell_variable(argument) {
                    variables += 1;
                    let assignment = format!("{variable}=");
                    let local_assignment = format!("local {variable}=");
                    let established = lines[..offset].iter().rposition(|line|
                        line.starts_with(&assignment) || line.starts_with(&local_assignment))
                        .map(|index| index + 1);
                    assert!(established.is_some_and(|line| line < call_line),
                        "{name}:{call_line}: {variable} must be assigned before import hook");
                } else {
                    // Do not silently treat an unsupported shell expansion as a literal.
                    assert!(!argument.contains('$'),
                        "{name}:{call_line}: unsupported verdict/status reference {argument}");
                }
            }
            if variables == 0 {
                let anchor = match (name, arguments[4], arguments[5]) {
                    ("run-aarch64-boot-test-strict.sh", "PASS", "0") =>
                        "if [ \"$SCORE_PASS\" = \"1\" ]; then",
                    ("run-aarch64-boot-test-strict.sh", "INCONCLUSIVE", "2") =>
                        "if [ \"$SCORE_STATUS\" -eq 2 ]; then",
                    ("run-aarch64-boot-test-strict.sh", "FAIL", "1") =>
                        "report_failure \"$iteration\"",
                    ("run-x86-boot-tests.sh", "PASS", "0") =>
                        "frame-custody gate run $i: PASS",
                    _ => panic!("{name}:{call_line}: literal outcome needs a placement anchor"),
                };
                let established = lines[..offset].iter().rposition(|line|
                    !line.starts_with('#') && line.contains(anchor))
                    .map(|index| index + 1);
                assert!(established.is_some_and(|line| line < call_line),
                    "{name}:{call_line}: outcome anchor {anchor:?} must precede import hook");
            }
        }
    }
}

/// Accept only a bare $NAME or "$NAME", with an exact shell identifier.
fn shell_variable(argument: &str) -> Option<&str> {
    let unquoted = argument.strip_prefix('"')
        .and_then(|value| value.strip_suffix('"')).unwrap_or(argument);
    let name = unquoted.strip_prefix('$')?;
    let mut chars = name.chars();
    let first = chars.next()?;
    if !(first.is_ascii_alphabetic() || first == '_')
        || !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return None;
    }
    Some(name)
}
