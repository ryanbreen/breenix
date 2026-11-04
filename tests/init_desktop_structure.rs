use std::fs;

#[test]
fn desktop_is_shared_and_does_not_start_gettys() {
    let source = fs::read_to_string("userspace/programs/src/init.rs").unwrap();
    assert_eq!(source.matches("fn run_desktop_mode()").count(), 1);
    let before = source.split("fn run_desktop_mode()").next().unwrap();
    assert!(!before.lines().rev().take(3).any(|line| line.contains("cfg(target_arch")));
    let desktop = source.split("fn run_desktop_mode()").nth(1).unwrap().split("\n}\n").next().unwrap();
    assert!(!desktop.contains("cfg(target_arch"));
    assert!(!desktop.contains("getty"));
    assert!(!desktop.contains("run_console_mode"));
    assert!(desktop.contains("run_boot_script()"));
    assert!(desktop.contains("Scope::Desktop"));
    assert!(desktop.contains("bwm"));
    assert!(desktop.contains("bterm"));
}

#[test]
fn supervised_bwm_forwards_measured_check_costs() {
    let source = fs::read_to_string("userspace/programs/src/init_services.rs").unwrap();
    let forwarding = source.split("let desktop = entry.def.name == \"bwm\"").nth(1).unwrap()
        .split("if output.stream").next().unwrap();
    assert!(forwarding.contains("line.starts_with(b\"VALUE bwm-check-us \")"));
}

#[test]
fn supervised_bwm_forwards_its_startup_banner() {
    let source = fs::read_to_string("userspace/programs/src/init_services.rs").unwrap();
    let forwarding = source.split("let desktop = entry.def.name == \"bwm\"").nth(1).unwrap()
        .split("if output.stream").next().unwrap();
    assert!(forwarding.contains("line.starts_with(b\"[bwm] Breenix Window Manager starting\")"));
}

#[test]
fn supervised_desktop_errors_are_not_hidden_by_stdout_filtering() {
    let source = fs::read_to_string("userspace/programs/src/init_services.rs").unwrap();
    let forwarding = source.split("let desktop = entry.def.name == \"bwm\"").nth(1).unwrap()
        .split("if output.stream").next().unwrap();
    for prefix in ["[bwm] ERROR:", "[bwm] GPU frame failed:", "[bwm] Discovered window",
        "[bwm] compositor acknowledgment failed:", "[bwm] display held by another process:"] {
        assert!(forwarding.contains(prefix), "missing {prefix}");
    }
    assert!(source.contains("if (output.stream == \"stdout\" && (readiness || oracle)) || desktop"));
}
