//! E9.1 dispatch must proceed when another CPU holds the scheduler list
//! lock, and must return to the interrupted context when its own queue is busy.

const SWITCH: &str = include_str!("../kernel/src/arch_impl/aarch64/context_switch.rs");

fn function<'a>(source: &'a str, name: &str) -> &'a str {
    let signature = format!("fn {name}(");
    let start = source.find(&signature).unwrap_or_else(|| panic!("missing {name}"));
    source[start..]
        .split_once("\n}")
        .unwrap_or_else(|| panic!("missing closing brace of {name}"))
        .0
}

#[test]
fn dispatch_does_not_take_the_scheduler_list_lock() {
    for entry in [
        "check_need_resched_and_switch_arm64",
        "schedule_from_kernel",
        "schedule_terminated_from_exit",
        "inline_schedule_trampoline",
        "exit_schedule_trampoline",
    ] {
        let body = function(SWITCH, entry);
        for line in body.lines().filter(|line| !line.trim_start().starts_with("//")) {
            assert!(
                !["lock_for_context_switch", "force_unlock_scheduler", "with_scheduler("]
                    .iter()
                    .any(|call| line.contains(call)),
                "{entry} is serialized with another CPU by the scheduler list lock: {line}"
            );
        }
    }
}
