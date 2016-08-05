
use alloc::boxed::Box;
use collections::Vec;

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

            let addr = ::test_call as *const () as usize;
            println!("Test call is at 0x{:x}", addr);

            asm!("push %rax" ::: "rax");
            asm!("push %rcx" ::: "rcx");
            asm!("push %rdx" ::: "rdx");
            asm!("push %r8" ::: "r8");
            asm!("push %r9" ::: "r9");
            asm!("push %r10" ::: "r10");
            asm!("push %r11" ::: "r11");
            asm!("push %rdi" ::: "rdi");
            asm!("push %rsi" ::: "rsi");

            //asm!("call $a" :: "a"(addr));

            asm!("pop %rsi" ::: "rsi");
            asm!("pop %rdi" ::: "rdi");
            asm!("pop %r11" ::: "r11");
            asm!("pop %r10" ::: "r10");
            asm!("pop %r9" ::: "r9");
            asm!("pop %r8" ::: "r8");
            asm!("pop %rdx" ::: "rdx");
            asm!("pop %rcx" ::: "rcx");
            asm!("pop %rax" ::: "rax");

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

    fn halt(&self) {
        unsafe {
            asm!("hlt");
            asm!("pause");
        }
    }
}
