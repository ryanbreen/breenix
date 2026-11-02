#[path = "../kernel/src/drivers/watchdog_round.rs"]
mod policy;
use policy::Transport;
const TIMER: [u8; 6] = [4, 1, 0, 0x10, 0xb0, 4];

struct Bmc { timer: [u8; 8], fail: Option<&'static str>, sets: usize, feeds: usize }
impl Bmc {
    fn new() -> Self { Self { timer: [0; 8], fail: None, sets: 0, feeds: 0 } }
    fn error(&mut self, operation: &str) -> Result<(), &'static str> {
        if self.fail == Some(operation) { self.fail = None; Err("transient KCS error") } else { Ok(()) }
    }
}
impl Transport for Bmc {
    type State = [u8; 8];
    fn configured(timer: &Self::State) -> bool {
        timer[0] & 0x47 == 0x44 && timer[1] == 1 && timer[2] == 0 && timer[4..6] == TIMER[4..6]
    }
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

#[derive(Clone, Copy, Default)]
struct Registers { enabled: bool, timeout: u32 }

#[derive(Default)]
struct RegisterWatchdog { registers: Registers, sets: usize, feeds: usize, mismatch: bool }
impl Transport for RegisterWatchdog {
    type State = Registers;
    fn configured(state: &Self::State) -> bool { state.enabled && state.timeout == 120 }
    fn get(&mut self) -> Result<Self::State, &'static str> {
        let mut state = self.registers;
        if self.mismatch && self.feeds > 0 { state.timeout = 0; }
        Ok(state)
    }
    fn set(&mut self) -> Result<(), &'static str> {
        self.sets += 1;
        self.registers = Registers { enabled: true, timeout: 120 };
        Ok(())
    }
    fn reset(&mut self) -> Result<(), &'static str> { self.feeds += 1; Ok(()) }
}

#[test]
fn register_transport_obeys_health_and_rearms_lost_configuration() {
    let mut watchdog = RegisterWatchdog::default();
    assert_eq!(policy::feed(&mut watchdog, || Ok(())), Ok(true));
    assert!(policy::feed(&mut watchdog, || Err("EIO")).is_err());
    assert_eq!(watchdog.feeds, 1);
    assert_eq!(policy::feed(&mut watchdog, || Ok(())), Ok(false));
    assert_eq!(watchdog.sets, 1);
    watchdog.registers = Registers::default();
    assert_eq!(policy::feed(&mut watchdog, || Ok(())), Ok(true));
    assert_eq!(watchdog.sets, 2);
    assert_eq!(watchdog.feeds, 3);
}

#[test]
fn register_transport_rejects_readback_mismatch() {
    let mut watchdog = RegisterWatchdog { mismatch: true, ..Default::default() };
    assert_eq!(policy::feed(&mut watchdog, || Ok(())), Err("watchdog settings readback"));
    assert_eq!(watchdog.feeds, 1);
}
