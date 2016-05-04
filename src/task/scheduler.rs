
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
    Scheduler {
      tasks: Box::new(Vec::new())
    }
  }

  pub fn add_task(&mut self, task: Task) {
    self.tasks.push(Box::new(task));
  }

  pub fn schedule(&mut self) {

  }

  pub fn disable_interrupts(&self) {
    asm!("cli");
  }

  pub fn enable_interrupts(&self) {
    asm!("sti");
  }

  pub fn idle(&self) -> ! {
    loop {
      self.halt();
    }
  }

  fn halt(&self) {
    unsafe {
      asm!("hlt");
      asm!("pause");
    }
  }
}
