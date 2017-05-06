
use alloc::boxed::Box;
use collections::Vec;

use memory;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Process {
    pid: usize,
    frame: usize, // physical address of this process's pml4
    allocated_pages: usize, // number of allocated pages

    state: usize, // -1 unrunnable, 0 runnable, >0 stopped
    stack: usize,
    usage: usize,
}

impl Process {
    fn new(pid: usize, frame:usize, stack: usize) -> Self {
        Process {
            pid: pid,
            frame: frame,
            allocated_pages: 0,
            state: stack,
            stack: 0,
            usage: 0,
        }
    }
}

#[allow(dead_code)]
pub struct Scheduler {
    procs: Vec<Process>,
    pid_counter: usize,
}

#[allow(dead_code)]
impl Scheduler {
    pub fn new() -> Self {
        let mut scheduler = Scheduler {
            procs: Vec::new(),
            pid_counter: 0,
        };

        scheduler.create_process(0, 0);

        scheduler
    }

    pub fn create_process(&mut self, memory_frame: usize, stack_pointer: usize) -> usize {

        self.pid_counter += 1;

        // Init proc 1 for the main kernel thread
        self.procs.push(Process::new(self.pid_counter, memory_frame, stack_pointer));

        println!("initialized proc {}", self.pid_counter);

        self.pid_counter
    }

    pub fn schedule(&mut self) -> usize {
        unsafe {
            self.disable_interrupts();
            self.test();
            self.enable_interrupts();
        }

        0
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

    fn super_inner(&self) -> usize {
        let mut beans = 0;

        for i in 0..1000 {
            beans += 1;
        }

        beans
    }

    #[inline(never)]
    fn inner(&self) {
        let mut x = 0;
        x += 100;
        println!("From new stack: {}", x);

        println!("{}", self.super_inner());
    }

    unsafe fn test(&self) {
        // Create a new stack
        let new_stack = memory::memory_controller().alloc_stack(64)
            .expect("could not allocate new proc stack");
        println!("Top of new stack: {:x}", new_stack.top());

        let sp:usize;

        // Jump to new stack
        asm!("movq %rsp, %rcx
              movq $1, %rsp" : "={rcx}"(sp) : "r"(new_stack.top()) : "rcx");
        
        self.inner();

        asm!("movq $0, %rsp" :: "r"(sp) : );
    }

    fn halt(&self) {
        unsafe {
            asm!("hlt");
            asm!("pause");
        }
    }
}
