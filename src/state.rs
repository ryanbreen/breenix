use arr_macro::arr;
use core::sync::atomic::{AtomicU64, Ordering};
use lazy_static::lazy_static;

//use io::pci::Device;
//use io::drivers::network::NetworkInterface;

//use task::scheduler::Scheduler;

pub struct State {
    interrupt_count: [AtomicU64; 256],
    //    pub scheduler: Scheduler,
    //    pub devices: Vec<Device>,
    //    pub network_interfaces: Vec<NetworkInterface>,
}

impl core::fmt::Debug for State {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(f, "State")?;
        for i in 0..256 {
            if self.interrupt_count[i].load(Ordering::Relaxed) > 0 {
                write!(
                    f,
                    "\n\tInterrupt {} count == {}",
                    i,
                    self.interrupt_count[i].load(Ordering::Relaxed)
                )?;
            }
        }

        Ok(())
    }
}

lazy_static! {

    pub static ref STATE: State = {
        State {
            interrupt_count: arr![AtomicU64::new(0); 256],
            //scheduler: Scheduler::new(),
            //devices: Vec::new(),
            //network_interfaces: Vec::new(),
        }
    };
}

pub fn increment_interrupt_count(interrupt: usize) {
    STATE.interrupt_count[interrupt].fetch_add(1, Ordering::Relaxed);
}

pub fn debug() {
    crate::debugln!("{:?}", *STATE);
}
