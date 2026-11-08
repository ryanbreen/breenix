//! Session display custody. Effects complete before the next owner is published.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner { Initial, SupervisorReady, CompositorAlive, CompositorDied, SupervisorHolds, CaseEnd, RunnerHolds }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event { Begin, Started, Died, Recover, End, Reclaim }

pub trait Custody {
    fn take_display(&mut self) -> Result<(), String>;
    fn claim_compositor(&mut self) -> Result<(), String>;
    fn release_compositor(&mut self) -> Result<(), String>;
    fn give_display(&mut self) -> Result<(), String>;
}
pub struct Ownership { owner: Owner, claimed: bool }
impl Default for Ownership {
    fn default() -> Self { Self { owner: Owner::Initial, claimed: false } }
}
impl Ownership {
    pub fn owner(&self) -> Owner { self.owner }
    pub fn apply(&mut self, event: Event, display: &mut impl Custody) -> Result<(), String> {
        use Event::*; use Owner::*;
        let next = match (self.owner, event) {
            (Initial, Begin) => { display.take_display()?; SupervisorReady }
            (SupervisorReady, Started) => CompositorAlive,
            (CompositorAlive, Died) => CompositorDied,
            (CompositorDied, Recover) => {
                display.take_display()?;
                display.claim_compositor()?;
                self.claimed = true;
                SupervisorHolds
            }
            (SupervisorReady | CompositorDied | SupervisorHolds, End) => {
                if self.claimed { display.release_compositor()?; self.claimed = false; }
                display.give_display()?;
                CaseEnd
            }
            (CaseEnd, Reclaim) => { display.take_display()?; RunnerHolds }
            _ => return Err(format!("illegal session ownership transition {:?} -> {event:?}", self.owner)),
        };
        self.owner = next;
        Ok(())
    }
}
