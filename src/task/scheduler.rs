
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

  fn switch(&self, current_task: &Task, new_task: &Task) {
    asm!("pushf");
    asm!("pushq %rbx");
    asm!("pushq %rbpv");
    asm!("pushq %r12");
    asm!("pushq %r13");
    asm!("pushq %r14");
    asm!("pushq %r15");

    asm!("movq %rsp,(%rdi)");
    asm!("movq %rsi,%rsp");

    asm!("popq %r15");
    asm!("popq %r14");
    asm!("popq %r13");
    asm!("popq %r12");
    asm!("popq %rbp");
    asm!("popq %rbx");
    asm!("popf");
  }

  pub fn idle(&self) {
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
