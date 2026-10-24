#[path = "../kernel/src/drivers/watchdog_round.rs"]
mod policy;
use policy::{Transport, TIMER};

struct Bmc { timer: [u8; 8], fail: Option<&'static str>, sets: usize, feeds: usize }
impl Bmc {
    fn new() -> Self { Self { timer: [0; 8], fail: None, sets: 0, feeds: 0 } }
    fn error(&mut self, operation: &str) -> Result<(), &'static str> {
        if self.fail == Some(operation) { self.fail = None; Err("transient KCS error") } else { Ok(()) }
    }
}
impl Transport for Bmc {
    fn get(&mut self) -> Result<[u8; 8], &'static str> { self.error("get")?; Ok(self.timer) }
    fn set(&mut self) -> Result<(), &'static str> {
        self.error("set")?;
        self.sets += 1;
        self.timer[..3].copy_from_slice(&TIMER[..3]);
        self.timer[4..6].copy_from_slice(&TIMER[4..6]);
        Ok(())
    }
    fn reset(&mut self) -> Result<(), &'static str> {
        self.error("reset")?; self.feeds += 1; self.timer[0] |= 0x40; Ok(())
    }
}
#[test]
fn health_failure_never_feeds_and_next_healthy_round_recovers() {
    let mut bmc = Bmc::new();
    assert_eq!(policy::feed(&mut bmc, || Ok(())), Ok(true));
    assert!(policy::feed(&mut bmc, || Err("EIO")).is_err());
    assert_eq!(bmc.feeds, 1);
    assert_eq!(policy::feed(&mut bmc, || Ok(())), Ok(false));
    assert_eq!(bmc.feeds, 2);
    assert_eq!(bmc.sets, 1);
}
#[test]
fn transient_transport_errors_and_bmc_restart_recover_after_health() {
    for operation in ["get", "set", "reset"] {
        let mut bmc = Bmc::new();
        bmc.fail = Some(operation);
        assert!(policy::feed(&mut bmc, || Ok(())).is_err());
        assert_eq!(bmc.feeds, 0);
        assert_eq!(policy::feed(&mut bmc, || Ok(())), Ok(true));
        assert_eq!(bmc.feeds, 1);
        bmc.timer = [0; 8]; // BMC restart loses configuration and running state.
        assert!(policy::feed(&mut bmc, || Err("EIO")).is_err());
        assert_eq!(bmc.feeds, 1);
        assert_eq!(policy::feed(&mut bmc, || Ok(())), Ok(true));
        assert_eq!(bmc.feeds, 2);
    }
}
