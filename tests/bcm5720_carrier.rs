#[path = "../kernel/src/drivers/bcm5720/carrier.rs"]
mod carrier;
use carrier::{Carrier, DOWN_SAMPLES};

#[test]
fn errors_are_unknown_and_break_the_down_sequence() {
    let mut link = Carrier::linked();
    for _ in 0..20 {
        link.sample(Err("MDIO timeout"));
        assert!(link.up);
    }
    assert_eq!(link.failures, 20);
    for _ in 1..DOWN_SAMPLES {
        link.sample(Ok(false));
        assert!(link.up);
    }
    assert_eq!(link.failures, 0);
    link.sample(Err("PHY read failed"));
    for _ in 1..DOWN_SAMPLES {
        link.sample(Ok(false));
        assert!(link.up);
    }
    link.sample(Ok(false));
    assert!(!link.up);
    link.sample(Err("PHY read failed"));
    assert!(!link.up);
    link.sample(Ok(true));
    assert!(link.up);
    assert_eq!(link.failures, 0);
}

#[test]
fn an_up_sample_cancels_transient_down_and_recovers_immediately() {
    let mut link = Carrier::linked();
    for _ in 0..10 {
        link.sample(Ok(false));
        link.sample(Ok(false));
        link.sample(Ok(true));
        assert!(link.up);
    }
    for _ in 0..DOWN_SAMPLES {
        link.sample(Ok(false));
    }
    assert!(!link.up);
    link.sample(Ok(true));
    assert!(link.up);
}
