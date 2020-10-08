use alloc::vec::Vec;

use crate::event::keyboard::KeyEvent;
use crate::event::keyboard::KeyEventHandler;

use lazy_static::lazy_static;
use spin::Mutex;

use crate::println;

//use io::pci::Device;
//use io::drivers::network::NetworkInterface;

//use task::scheduler::Scheduler;

const interrupt_count: [u64; 256] = [0; 256];

pub struct State {
    pub key_listeners: Vec<KeyEventHandler>,
//    pub scheduler: Scheduler,
//    pub devices: Vec<Device>,
//    pub network_interfaces: Vec<NetworkInterface>,
}

impl core::fmt::Debug for State {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {

        write!(f, "State");
        for i in 0..256 {
            if (interrupt_count[i] > 0) {
                write!(f, "\n\tInterrupt {} count == {}", i, interrupt_count[i]);
            }
        }

        Ok(())
    }
}

lazy_static! {

    pub static ref STATE: Mutex<State> = {
        let state = State {
            key_listeners: Vec::new(),
            //scheduler: Scheduler::new(),
            //devices: Vec::new(),
            //network_interfaces: Vec::new(),
        };

        Mutex::new(state)
    };
}

pub fn increment_interrupt_count(interrupt:usize) {
    interrupt_count[interrupt] += 1;
}

pub fn register_key_event_listener(listener: KeyEventHandler) {
    STATE.lock().key_listeners.push(listener);
    crate::println!("There are now {} key listeners",
             STATE.lock().key_listeners.len());
}

pub fn dispatch_key_event(ev: &KeyEvent) {
    let listeners = &(STATE.lock().key_listeners);
    for listener in listeners {
        if (&listener.handles_event)(ev) {
            (&listener.notify)(ev);
        }
    }
}

pub fn debug() {
    use x86_64::instructions::interrupts;
    interrupts::without_interrupts(|| {
        crate::println!("{:?}", *(STATE.lock()));
    });
}
