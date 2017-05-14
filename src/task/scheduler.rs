
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
    pub procs: BTreeMap<usize, Process>,
    pub current: usize,
    pid_counter: usize,
    skip_count: usize,
}

#[inline(never)]
fn test() {

    unsafe {
        asm!("sti");
    }

    let mut beans = 0;

    loop {
        beans += 1;

        if beans % 10000000 == 0 {
            println!("{} {}", beans, ::state().scheduler.procs.len());
        }
    }

    //asm!("movq $0, %rsp" :: "r"(sp) : );
}

#[allow(dead_code)]
impl Scheduler {
    pub fn new() -> Self {
        let mut scheduler = Scheduler {
            procs: BTreeMap::new(),
            current: 0,
            pid_counter: 0,
            skip_count: 0,
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
        bootstrap_println!("inserted proc {}, there are {} procs", self.pid_counter, self.procs.len());
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

    pub fn get_next_process(&mut self) -> Option<usize> {

        if self.procs.len() == 1 {
            return None;
        }

        if self.skip_count >= self.procs.len() - 1 {
            self.skip_count = 0;
        }

        let mut i = 0;

        for (&pid, _) in &self.procs {

            i += 1;

            if pid == self.current { // || /* testing */ pid == 0 {
                continue;
            }

            if i > self.skip_count {
                self.skip_count += 1;
                self.current = pid;
                return Some(pid);
            }
        }

        None
    }

    pub fn start_new_process(&mut self, fn_ptr: usize) {
        let pid = self.create_process(fn_ptr, 0);
        let mut process = self.get_process(pid);

        // Create a new stack
        let new_stack = memory::memory_controller().alloc_stack(1)
            .expect("could not allocate new proc stack");
        println!("Top of new stack: {:x}", new_stack.top());

        // Set process to have pointer to its stack and function
        process.stack = new_stack.top();
        process.start_pointer = fn_ptr;
    }

    fn switch(&mut self) {

        // Find a new process to run
        let process_opt = self.get_next_process();
        if process_opt.is_none() {
            // The running proc is the only one available.
            return;
        }

        let pid = process_opt.unwrap();
        let mut process = self.get_process(pid);

        if process.started {
            // jump to trap frame
            //println!("Jumping to 0x{:x}", process.trap_frame);
            unsafe {
                asm!(  "movq $0, %rsp
                        pop    %rax
                        pop    %rbx
                        pop    %rcx
                        pop    %rdx
                        pop    %rsi
                        pop    %rdi
                        pop    %r8
                        pop    %r9
                        pop    %r10
                        pop    %r11
                        pop    %rbp
                        sti
                        iretq" : /* no outputs */ : "r"(process.trap_frame) : );
            }
        } else {
            // call init fn
            println!("Attempting to start process at {:x}", process.start_pointer);

            unsafe {
                process.started = true;

                asm!("movq $0, %rsp
                      jmpq *$1" :: "r"(process.stack), "r"(test as usize) : );

                // TODO: free stack

                // If we fell through to here, the process is over.

                use util::syscall;
                syscall::syscall0(/* exit */ 60);
            }
        }
    }

    pub fn update_trap_frame(&mut self, pointer: usize) {
        self.get_current_process().trap_frame = pointer;

        if self.current != 0 {
            //println!("Updated trap frame pointer for proc {} to {:x}", self.current, pointer);
        }
    }

    pub fn create_test_process(&mut self) {

        self.disable_interrupts();

        self.start_new_process(test as usize);
        self.start_new_process(test as usize);
        self.start_new_process(test as usize);
        self.start_new_process(test as usize);
        
        self.enable_interrupts();
    }

    pub fn schedule(&mut self) -> usize {
        self.switch();

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
            println!("halt");
            asm!("hlt");
            asm!("pause");
        }
    }
}
