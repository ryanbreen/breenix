
use alloc::boxed::Box;
use collections::Vec;

use memory;

use task::Task;

#[allow(dead_code)]
pub struct Scheduler {
    tasks: Box<Vec<Box<Task>>>,
}

#[allow(dead_code)]
impl Scheduler {
    pub fn new() -> Self {
        Scheduler { tasks: Box::new(Vec::new()) }
    }

    pub fn add_task(&mut self, task: Task) {
        self.tasks.push(Box::new(task));
    }

    pub fn schedule(&mut self) {
        unsafe {
            self.disable_interrupts();
            self.test();
            self.enable_interrupts();
        }
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
