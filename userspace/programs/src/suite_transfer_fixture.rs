//! Test-root SSH fixture. The host pins bsshd's key from serial and ends the
//! lease through the operator fixture's normal bsh exec path.

use breenix_suite::suite::{case, category, suite, CaseResult, Suite};

#[path = "fixture/ssh_lease.rs"]
mod ssh_lease;

static SUITE: Suite = suite(
    "transfer-fixture",
    "Transfer fixture",
    &[category(
        "fixture",
        "Test-boot listener",
        &[case(
            "listener-lease",
            "The boot host completes the restricted SSH lease",
            listener_lease,
        )],
    )],
);

fn main() {
    SUITE.run()
}

fn listener_lease() -> CaseResult {
    ssh_lease::with_listener("TRANSFER READY", "TRANSFER LEASE-END", |_, _| Ok(()))
}
