
pub struct Scheduler {
  tasks: Box<Vec<Task>>,
}

impl Scheduler {
  pub fn add_task(&mut self, task: &Task) {
    self.tasks.push(Box::new(task));
  }

  pub fn schedule(&mut self) {
    
  }
}
