
use alloc::boxed::Box;
use collections::Vec;

use task::Task;

#[allow(dead_code)]
pub struct Scheduler {
    tasks: Box<Vec<Box<Task>>>,
}

#[allow(dead_code)]
impl Scheduler {
    pub fn new() -> Self {
        Scheduler { tasks: Box::new(Vec::new()) }
    }

    pub fn add_task(&mut self, task: Task) {
        self.tasks.push(Box::new(task));
    }

    pub fn schedule(&mut self) {
        unsafe {
            //self.disable_interrupts();

            //self.enable_interrupts();
        }
    }

    pub fn disable_interrupts(&self) {
        unsafe {
            asm!("cli");
        }
    }

    pub fn enable_interrupts(&self) {
        unsafe {
            asm!("sti");
        }
    }

    pub fn idle(&self) -> ! {
        loop {
            self.halt();
        }
    }

    unsafe fn test(&self) {

    }

    fn halt(&self) {
        unsafe {

            self.test();

            asm!("hlt");
            asm!("pause");
        }
    }
}
