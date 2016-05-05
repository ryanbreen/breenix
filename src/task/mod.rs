
pub mod scheduler;

/// We plan to follow the Linux model where possible, 

#[allow(dead_code)]
pub struct Task {
  state: u64,    /* -1 unrunnable, 0 runnable, >0 stopped */
  stack: u64,
  usage: u64,
  flags: u32,     /* per process flags, defined below */
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Process<'a> {
  pid: usize,
  frame: usize, /* physical address of this process's pml4*/
  allocated_pages: usize, /* number of allocated pages */

  /* linked list of processes */
  next: Option<&'a Process<'a>>,
  previous: Option<&'a Process<'a>>
}

static mut PID_COUNTER:usize = 0;

pub fn create_process<'a>() -> Process<'a> {
  unsafe {
    PID_COUNTER += 1;

    Process {
      pid: PID_COUNTER,
      frame: 0,
      allocated_pages: 0,
      next: None,
      previous: None,
    }
  }
}