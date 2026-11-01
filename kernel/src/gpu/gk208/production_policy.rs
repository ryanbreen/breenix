//! Production clock policy (K8). Hardware mechanisms live in the shared C3-C5 helpers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage { Sensor, Pmu, Ram, MemoryIntermediate, MemoryTarget, Gpc, Verify }
impl Stage {
    pub const ALL: [Self; 7] = [Self::Sensor, Self::Pmu, Self::Ram,
        Self::MemoryIntermediate, Self::MemoryTarget, Self::Gpc, Self::Verify];
}
pub trait Startup {
    fn step(&mut self, stage: Stage) -> Result<(), &'static str>;
    fn changed(&self) -> (bool,bool);
    fn restore_gpc(&mut self) -> Result<(), &'static str>;
    fn restore_memory(&mut self) -> Result<(), &'static str>;
}
pub struct Outcome {
    pub full: bool,
    pub failure: Option<&'static str>,
    pub failed_stage: Option<Stage>,
    pub gpc_restore: Option<&'static str>,
    pub memory_restore: Option<&'static str>,
}
pub fn start(io: &mut impl Startup, keep_post: bool) -> Outcome {
    let mut out = Outcome { full: false, failure: None, failed_stage: None, gpc_restore: None, memory_restore: None };
    if keep_post { return out; }
    for stage in Stage::ALL {
        if let Err(reason) = io.step(stage) {
            out.failure = Some(reason);
            out.failed_stage = Some(stage);
            let (gpc,memory)=io.changed();
            // Attempt the second domain even when the first restore fails.
            if gpc { out.gpc_restore = io.restore_gpc().err(); }
            if memory { out.memory_restore = io.restore_memory().err(); }
            return out;
        }
    }
    out.full = true;
    out
}
pub fn within_one_percent(value: Result<u32, &'static str>, target: u32) -> bool {
    value.is_ok_and(|v| v.abs_diff(target) <= target / 100)
}
pub struct Guard { tripped: bool, next_restore:u64, stopped:bool }
impl Guard {
    pub const fn new() -> Self { Self { tripped: false, next_restore:0, stopped:false } }
    pub fn sample(&mut self, temperature: Result<u32, &'static str>) -> bool {
        if self.tripped { return false; }
        if temperature.is_err() || temperature.is_ok_and(|v| v >= 85) {
            self.tripped = true;
            return true;
        }
        false
    }
    pub fn retry_due(&mut self,now:u64,armed:bool)->bool {
        if !self.tripped || !armed || now<self.next_restore {return false;}
        self.next_restore=now.saturating_add(1_000_000_000);true
    }
    pub fn critical(&mut self,temperature:Result<u32,&'static str>,armed:bool)->bool {
        if self.stopped || !armed || !temperature.is_ok_and(|v|v>=95) {return false;}
        self.stopped=true;true
    }
    pub fn tripped(&self) -> bool { self.tripped }
}

#[derive(Debug)]
pub struct HealthFailure {pub check:&'static str,pub register:u32,pub value:u32}
/// GR PRI is available after PMC enables PGRAPH; PMC/PFIFO/PMU remain readable
/// before GPU admission (the same accessibility rule as gr/ctxctl.rs).
pub fn clock_health(read:&mut impl FnMut(u32)->u32)->Result<(),HealthFailure> {
 let disp=read(0x10a01c);let raw=read(0x10a008);let pending=raw&disp&!(disp>>16);
 if pending&!0xc0!=0 {return Err(HealthFailure {check:"pmu-pending",register:0x10a008,value:raw});}
 let fifo=read(0x259c);
 if fifo!=0 {return Err(HealthFailure {check:"fifo-fault",register:0x259c,value:fifo});}
 if read(0x200)&0x1000!=0 {
  let gr=read(0x400100);
  if gr!=0 {return Err(HealthFailure {check:"pgraph-fault",register:0x400100,value:gr});}
 }
 Ok(())
}
