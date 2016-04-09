
use alloc::boxed::Box;
use collections::Vec;

use task::Task;

pub struct Scheduler {
  tasks: Box<Vec<Box<Task>>>,
}

impl Scheduler {
  pub fn new() -> Self {
    Scheduler {
      tasks: Box::new(Vec::new())
    }
  }

  pub fn add_task(&mut self, task: Task) {
    self.tasks.push(Box::new(task));
  }

  pub fn schedule(&mut self) {

  }

  pub fn idle(&self) {
    unsafe {
      println!("Entering idle");

      asm!("sti");
      asm!("hlt");
      asm!("cli");

      println!("No longer halting");
    }
  }
}
