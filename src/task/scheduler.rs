
use alloc::boxed::Box;
use collections::BTreeMap;
use collections::Vec;

use memory;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Process {
    pid: usize,
    frame: usize, // physical address of this process's pml4
    allocated_pages: usize, // number of allocated pages

    started: bool,
    start_pointer: usize,
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
            started: false,
            start_pointer: 0,
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

unsafe fn test() {
    
/*
            // Jump to new stack and pc
            asm!("movq %rsp, %rcx
                  movq $1, %rsp" : "={rcx}"(sp) : "r"(new_stack.top()) : "rcx");
*/
    let mut beans = 0;
    beans += 1000;

    println!("{}", beans);

    //::state().scheduler.idle();

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

        let pid = scheduler.create_process(0, 0);
        scheduler.current = 0;

        {
            let process = scheduler.get_process(pid);
            process.started = true;
            process.state = 1;
        }

        scheduler
    }

    pub fn create_process(&mut self, memory_frame: usize, stack_pointer: usize) -> usize {

        // Init proc 0 for the main kernel thread
        let p = Process::new(self.pid_counter, memory_frame, stack_pointer);
        self.procs.insert(self.pid_counter, p);
        let pid = self.pid_counter;
        self.pid_counter += 1;
        pid
    }

    pub fn get_current_process(&mut self) -> &mut Process {
        let pid = self.current;
        self.get_process(pid)
    }

    pub fn get_process(&mut self, pid: usize) -> &mut Process {
        self.procs.get_mut(&pid).unwrap()
    }

    pub fn start_new_process(&mut self, fn_ptr: usize) {
        let pid = self.create_process(fn_ptr, 0);
        let mut process = self.get_process(pid);

        // Create a new stack
        let new_stack = memory::memory_controller().alloc_stack(64)
            .expect("could not allocate new proc stack");
        println!("Top of new stack: {:x}", new_stack.top());

        // Set process to have pointer to its stack and function
        process.stack = new_stack.top();
        process.start_pointer = fn_ptr;

        // Then jump back to the original thread's stack and continue

        /*
        let orig_sp:usize;
        asm!("movq %rsp, %rcx
              movq $0, %rsp" : "={rcx}"(orig_sp): "r"(new_stack.top()) : "rcx" );
              */

        // Jump to new stack and pc;

        println!("Fell back to pid 0, so frobbing the wumpus");
        //asm!("movq $0, %rsp" :: "r"(orig_sp) : "rcx" );
    }

    fn switch(&mut self) {

        {
            self.current = 1;
        }

        // Find a new process to run
        let mut process = self.get_process(1);

        if process.started {
            // jump to trap frame
            unsafe {
                asm!("movq $0, %rip
                      iretq" : /* no outputs */ : "r"(process.trap_frame) : );
            }
        } else {
            // call init fn
            println!("Attempting to start process at {:x}", process.start_pointer);

            unsafe {
                asm!("call *$0" :: "r"(process.start_pointer) : );

                // if we fall through to here, the proc has exited

                // TODO: free stack

                use util::syscall;
                syscall::syscall0(/* exit */ 60);
            }
        }
    }

    pub fn update_trap_frame(&mut self, pointer: usize) {
        self.get_current_process().trap_frame = pointer;

        println!("Updated trap frame pointer for proc {} to {:x}", self.current, pointer);
    }

    pub fn create_test_process(&mut self) {
        self.start_new_process(test as usize);
    }

    pub fn schedule(&mut self) -> usize {
        unsafe {
            self.switch();
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

    fn halt(&self) {
        unsafe {
            asm!("hlt");
            asm!("pause");
        }
    }
}
