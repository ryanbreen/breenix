//! Native register capture and landing-pad transfer.

#[cfg(target_arch = "aarch64")]
mod aarch64;
#[cfg(target_arch = "x86_64")]
mod x86_64;

#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::*;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::*;

#[cfg(target_os = "breenix")]
pub(crate) type NativeContext = crate::level1::Context<REGISTER_COUNT>;

#[cfg(target_os = "breenix")]
impl RegisterContext {
    pub(crate) fn cursor(&self) -> NativeContext {
        let registers = core::array::from_fn(|index| {
            is_captured_register(index).then_some(self.registers[index])
        });
        NativeContext::new(registers, self.registers[IP], false)
    }

    pub(crate) fn landing_pad(cursor: &NativeContext) -> Option<Self> {
        let mut registers = core::array::from_fn(|index| cursor.get_gr(index as u64).unwrap_or(0));
        // Restoration requires the ABI's nonvolatile registers and a live SP.
        for index in NONVOLATILE.iter().copied().chain([SP]) {
            registers[index] = cursor.get_gr(index as u64)?;
        }
        registers[IP] = cursor.get_ip();
        (registers[IP] != 0 && registers[SP] != 0).then_some(Self { registers })
    }
}

#[cfg(target_os = "breenix")]
pub(crate) fn initial_rules<'a>() -> crate::cfi::Rules<'a, REGISTER_COUNT> {
    use crate::cfi::{RegisterRule, Rules};

    let mut rules = Rules::default();
    for &register in NONVOLATILE {
        rules.registers[register] = RegisterRule::SameValue;
    }
    // A call's CFA is the caller's stack pointer unless the CFI overrides it.
    rules.registers[SP] = RegisterRule::ValOffset(0);
    #[cfg(target_arch = "aarch64")]
    {
        // A leaf can leave the return address in LR (AAPCS64 §6.5).
        rules.registers[30] = RegisterRule::SameValue;
    }
    rules
}
