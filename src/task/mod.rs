/// We plan to follow the Linux model where possible, 

struct Task {
  state: u64,    /* -1 unrunnable, 0 runnable, >0 stopped */
  stack: u64,
  usage: u64,
  flags: u32,     /* per process flags, defined below */
}