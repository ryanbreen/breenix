use x86_64::structures::idt::InterruptStackFrame;
use x86_64::VirtAddr;

use crate::arch_impl::traits::InterruptFrame;

use super::privilege::X86PrivilegeLevel;

pub struct X86InterruptFrame(pub InterruptStackFrame);

impl InterruptFrame for X86InterruptFrame {
    type Privilege = X86PrivilegeLevel;

    fn instruction_pointer(&self) -> u64 {
        self.0.instruction_pointer.as_u64()
    }

    fn stack_pointer(&self) -> u64 {
        self.0.stack_pointer.as_u64()
    }

    fn set_instruction_pointer(&mut self, addr: u64) {
        let mut frame = unsafe { self.0.as_mut() };
        frame
            .map_mut(|frame| &mut frame.instruction_pointer)
            .write(VirtAddr::new(addr));
    }

    fn set_stack_pointer(&mut self, addr: u64) {
        let mut frame = unsafe { self.0.as_mut() };
        frame
            .map_mut(|frame| &mut frame.stack_pointer)
            .write(VirtAddr::new(addr));
    }

    fn privilege_level(&self) -> Self::Privilege {
        match self.0.code_segment.rpl() {
            x86_64::PrivilegeLevel::Ring3 => X86PrivilegeLevel::Ring3,
            _ => X86PrivilegeLevel::Ring0,
        }
    }
}
