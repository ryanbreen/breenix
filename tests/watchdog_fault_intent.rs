#[path = "../kernel/src/drivers/watchdog_fault_intent.rs"]
mod watchdog_fault_intent;

use watchdog_fault_intent::{parse, Mode};

#[test]
fn parses_fault_intent() {
    assert_eq!(parse(b"feeder-hang\n"), Ok(Some(Mode::FeederHang)));
    assert_eq!(parse(b"health-hang\n"), Ok(Some(Mode::HealthHang)));
    assert_eq!(parse(b""), Ok(None));
    assert!(parse(&[b'x'; 33]).is_err());
    assert!(parse(b"feeder-hang").is_err());
}
