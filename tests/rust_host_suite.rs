use serde_json::Value;

#[test]
fn case_sizes_from_x0_9() {
    let profile: Value =
        serde_json::from_str(include_str!("../toolchain/rust-host/x0/9.json")).unwrap();
    let runs = profile["crates"].as_object().expect("X0.9 crate profiles");
    let suite = include_str!("../userspace/programs/src/suite_rust_host.rs");

    for (constant, field) in [
        ("ANON_BYTES", "largest_anonymous_mapping_bytes"),
        ("FILE_BYTES", "largest_file_mapping_bytes"),
        ("THREAD_COUNT", "peak_threads"),
    ] {
        let maximum = runs
            .values()
            .flat_map(|runs| runs.as_array().expect("X0.9 runs for a crate"))
            .map(|run| {
                run[field]
                    .as_u64()
                    .unwrap_or_else(|| panic!("missing {field}"))
            })
            .max()
            .expect("X0.9 contains at least one run");
        let prefix = format!("const {constant}: usize = ");
        let actual: u64 = suite
            .lines()
            .find_map(|line| line.strip_prefix(&prefix)?.strip_suffix(';'))
            .unwrap_or_else(|| panic!("suite has no {constant} constant"))
            .replace('_', "")
            .parse()
            .unwrap_or_else(|_| panic!("{constant} must be an integer literal"));
        assert_eq!(
            actual, maximum,
            "{constant} must match the X0.9 {field} maximum"
        );
    }
}
