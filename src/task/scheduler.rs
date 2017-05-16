
use alloc::boxed::Box;
use collections::BTreeMap;
use collections::Vec;

use memory;

use spin::Mutex;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Process {
    pid: usize,
    //frame: usize, // physical address of this process's pml4
    allocated_pages: usize, // number of allocated pages

    started: bool,
    start_pointer: usize,
    state: usize, // -1 unrunnable, 0 runnable, >0 stopped
    stack: usize,
    trap_frame: usize,
    usage: usize,
}

impl Process {
    fn new(pid: usize, start_fn: usize, stack: usize) -> Self {
        Process {
            pid: pid,
            allocated_pages: 0,
            started: false,
            start_pointer: start_fn,
            state: 0,
            trap_frame: 0,
            stack: stack,
            usage: 0,
        }
    }
}

#[allow(dead_code)]
pub struct Scheduler {
    procs: Mutex<BTreeMap<usize, Process>>,
    pub current: usize,
    pid_counter: usize,
    skip_count: usize,
}

#[inline(never)]
fn test() {

    let mut beans = 0;

    loop {
        beans += 1;

        if beans % 10000000 == 0 {
            println!("{}", beans);
        }
    }

    //asm!("movq $0, %rsp" :: "r"(sp) : );
}

#[allow(dead_code)]
impl Scheduler {
    pub fn new() -> Self {
        let mut scheduler = Scheduler {
            procs: Mutex::new(BTreeMap::new()),
            current: 0,
            pid_counter: 0,
            skip_count: 0,
        };

        scheduler
    }

    pub fn init(&mut self) {

        let pid = self.create_process(0, 0);
        self.current = 0;
        self.set_started(pid);

        {
            let procs = self.procs.lock();
            let process = procs.get(&pid);
            bootstrap_println!("Initted proc 0 to {:?}", process);
        }
    }

    pub fn start_new_process(&mut self, fn_ptr: usize) {
        // Create a new stack
        let new_stack = memory::memory_controller().alloc_stack(64)
            .expect("could not allocate new proc stack");
        bootstrap_println!("Top of new stack: {:x}", new_stack.top());
        self.create_process(fn_ptr, new_stack.top());
    }

    pub fn create_process(&mut self, start_fn: usize, stack_pointer: usize) -> usize {

        let pid;
        self.disable_interrupts();
        {
            // Init proc 0 for the main kernel thread
            let p = Process::new(self.pid_counter, start_fn, stack_pointer);
            self.procs.lock().insert(self.pid_counter, p);
            bootstrap_println!("inserted proc {}, there are {} procs", self.pid_counter, self.procs.lock().len());
            pid = self.pid_counter;
            self.pid_counter += 1;
        }
        pid
    }

    pub fn set_started(&mut self, pid: usize) {
        self.disable_interrupts();
        {
            let mut procs = self.procs.lock();
            let p = procs.get_mut(&pid);
            match p {
                None => panic!("Unable to find process {}", pid),
               Some(process) => (*process).started = true,
            };
        }
    }

    pub fn update_trap_frame(&mut self, trap_frame: usize) {
        self.disable_interrupts();
        {
            let mut procs = self.procs.lock();
            let p = procs.get_mut(&self.current);
            match p {
                None => panic!("Unable to find process {}", self.current),
                Some(process) => (*process).trap_frame = trap_frame,
            };
        }
    }

    pub fn get_next_pid(&self) -> Option<usize> {

        let pid;
        self.disable_interrupts();
        {
            let procs = self.procs.lock();

            if procs.len() == 1 {
                return None;
            }

            pid = match self.current {
                0 => Some(1),
                1 => Some(2),
                2 => Some(1),
                _ => panic!("Oh no!")
            };
        }
        pid
    }

    fn switch(&mut self) {

        // Find a new process to run
        let process;

        self.disable_interrupts();
        {
            let pid_opt = self.get_next_pid();

            match pid_opt {
                Some(p) => {
                    let mut proc_table = self.procs.lock();
                    match proc_table.get_mut(&p) {
                        Some(prc) => {
                            process = prc.clone();
                            if !process.started {
                                (*prc).started = true;
                            }
                        },
                        None => panic!("Unable to find process {}", p),
                    }
                },
                None => return,
            };
        }

        self.current = process.pid;

        if process.started {
            // jump to trap frame
            //println!("Jumping to 0x{:x}", process.trap_frame);

            if process.pid == 1 {
                //println!("pidiful");
            }

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
            
            println!("Attempting to start process at {:x} {}", process.start_pointer, process.pid);

            unsafe {

                asm!("movq $0, %rsp
                      sti
                      jmpq *$1" :: "r"(process.stack), "r"(test as usize) : );

                // TODO: free stack

                // If we fell through to here, the process is over.

                use util::syscall;
                syscall::syscall0(/* exit */ 60);
            }
        }
    }

    pub fn create_test_process(&mut self) {

        self.start_new_process(test as usize);
        self.start_new_process(test as usize);
        /*
        self.start_new_process(test as usize);
        self.start_new_process(test as usize);
        */
    }

    pub fn schedule(&mut self) -> usize {
        self.switch();

        0
    }

    pub fn disable_interrupts(&self) {
        unsafe {
            use interrupts;
            if interrupts::test_passed {
                asm!("cli");
            }
        }
    }

    pub fn enable_interrupts(&self) {
        unsafe {
            use interrupts;
            if interrupts::test_passed {
                asm!("sti");
            }
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
