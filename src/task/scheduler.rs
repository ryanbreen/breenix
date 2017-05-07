
use alloc::boxed::Box;
use collections::BTreeMap;
use collections::Vec;

use memory;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Process {
    pid: usize,
    frame: usize, // physical address of this process's pml4
    allocated_pages: usize, // number of allocated pages

    state: usize, // -1 unrunnable, 0 runnable, >0 stopped
    stack: usize,
    trap_frame: usize,
    usage: usize,
}

impl Process {
    fn new(pid: usize, frame:usize, stack: usize) -> Self {
        Process {
            pid: pid,
            frame: frame,
            allocated_pages: 0,
            state: stack,
            trap_frame: 0,
            stack: 0,
            usage: 0,
        }
    }
}

#[allow(dead_code)]
pub struct Scheduler {
    procs: BTreeMap<usize, Process>,
    pub current: usize,
    pid_counter: usize,
}

#[naked]
unsafe fn test() {
    
/*
            // Jump to new stack and pc
            asm!("movq %rsp, %rcx
                  movq $1, %rsp" : "={rcx}"(sp) : "r"(new_stack.top()) : "rcx");
*/
    let mut beans = 0;
    beans += 1000;

    println!("{}", beans);

    ::state().scheduler.idle();

    //asm!("movq $0, %rsp" :: "r"(sp) : );
}

#[allow(dead_code)]
impl Scheduler {
    pub fn new() -> Self {
        let mut scheduler = Scheduler {
            procs: BTreeMap::new(),
            current: 0,
            pid_counter: 0,
        };

        scheduler.current = scheduler.create_process(0, 0);

        scheduler
    }

    pub fn create_process(&mut self, memory_frame: usize, stack_pointer: usize) -> usize {

        // Init proc 0 for the main kernel thread
        self.procs.insert(self.pid_counter, Process::new(self.pid_counter, memory_frame, stack_pointer));

        let pid = self.pid_counter;
        bootstrap_println!("initialized proc {}", pid);

        self.pid_counter += 1;

        pid
    }

    pub unsafe fn start_new_process(&mut self, fn_ptr: usize) {
        let pid = self.create_process(fn_ptr, 0);

        self.current = pid;

        // Create a new stack
        let new_stack = memory::memory_controller().alloc_stack(64)
            .expect("could not allocate new proc stack");
        println!("Top of new stack: {:x}", new_stack.top());

        let sp:usize;

        // Jump to new stack and pc;
        println!("Attempting to start process at {:x}", fn_ptr);
        asm!("movq $0, %rsp
              call *$1" :: "r"(new_stack.top()), "r"(fn_ptr) : );

        println!("Fell back to pid 0");
    }

    pub fn update_trap_frame(&mut self, pointer: usize) {
        self.procs.get_mut(&self.current).unwrap().trap_frame = pointer;

        println!("Updated trap frame pointer for proc {} to {:x}", self.current, pointer);
    }

    pub fn schedule(&mut self) -> usize {
        unsafe {
            //self.test();
            self.start_new_process(test as usize);
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

    fn halt(&self) {
        unsafe {
            asm!("hlt");
            asm!("pause");
        }
    }
}
