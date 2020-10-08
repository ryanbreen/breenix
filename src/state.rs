use crate::event::keyboard::KeyEvent;
use crate::event::keyboard::KeyEventHandler;

use lazy_static::lazy_static;
use spin::Mutex;

use crate::println;

//use io::pci::Device;
//use io::drivers::network::NetworkInterface;

//use task::scheduler::Scheduler;

pub struct State {
    interrupt_count: [u64; 256],
//    pub scheduler: Scheduler,
//    pub devices: Vec<Device>,
//    pub network_interfaces: Vec<NetworkInterface>,
}

impl core::fmt::Debug for State {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {

        write!(f, "State");
        for i in 0..256 {
            if (self.interrupt_count[i] > 0) {
                write!(f, "\n\tInterrupt {} count == {}", i, self.interrupt_count[i]);
            }
        }

        Ok(())
    }
}

lazy_static! {

    pub static ref STATE: Mutex<State> = {
        let state = State {
            interrupt_count: [0; 256],
            //scheduler: Scheduler::new(),
            //devices: Vec::new(),
            //network_interfaces: Vec::new(),
        };

        Mutex::new(state)
    };
}

pub fn increment_interrupt_count(interrupt:usize) {
    use x86_64::instructions::interrupts;
    interrupts::without_interrupts(|| {
        STATE.lock().interrupt_count[interrupt] += 1;
    });
}

pub fn debug() {
    use x86_64::instructions::interrupts;
    interrupts::without_interrupts(|| {
        crate::debugln!("{:?}", *(STATE.lock()));
    });
}
