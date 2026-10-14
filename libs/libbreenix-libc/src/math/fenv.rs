//! <fenv.h> (ISO C17 7.6) on the hardware's own registers.
//!
//! aarch64: the rounding direction is FPCR.RMode (bits 23:22) and the exception flags are
//! FPSR's cumulative bits 0..4. x86-64: SSE arithmetic uses MXCSR and long double uses
//! the x87 unit, so the rounding direction is set in both control registers and the
//! flags are the union of both status registers. Exception traps stay disabled, so every
//! function here only reads and writes registers. The constants are Linux's per-arch
//! values, which are the hardware bit positions.

#[cfg(target_arch = "aarch64")]
mod arch {
    // Only x86's software conversions raise these two from Rust; C sees them in fenv.h.
    #[allow(dead_code)]
    pub(crate) const FE_INVALID: i32 = 1;
    #[allow(dead_code)]
    pub(crate) const FE_DIVBYZERO: i32 = 2;
    pub(crate) const FE_OVERFLOW: i32 = 4;
    pub(crate) const FE_UNDERFLOW: i32 = 8;
    pub(crate) const FE_INEXACT: i32 = 16;
    pub(crate) const FE_ALL_EXCEPT: i32 = 31;
    pub(crate) const FE_TONEAREST: i32 = 0;
    pub(crate) const FE_UPWARD: i32 = 0x40_0000;
    pub(crate) const FE_DOWNWARD: i32 = 0x80_0000;
    pub(crate) const FE_TOWARDZERO: i32 = 0xc0_0000;
    const RMODE: u64 = 0xc0_0000;

    /// fexcept_t.
    pub type FExcept = u32;

    /// fenv_t: { unsigned int fpcr; unsigned int fpsr; }.
    #[repr(C)]
    pub struct FEnv {
        fpcr: u32,
        fpsr: u32,
    }

    fn fpcr() -> u64 {
        let v: u64;
        unsafe { core::arch::asm!("mrs {}, fpcr", out(reg) v, options(nomem, nostack)) };
        v
    }
    fn set_fpcr(v: u64) {
        unsafe { core::arch::asm!("msr fpcr, {}", in(reg) v, options(nomem, nostack)) };
    }
    fn fpsr() -> u64 {
        let v: u64;
        unsafe { core::arch::asm!("mrs {}, fpsr", out(reg) v, options(nomem, nostack)) };
        v
    }
    fn set_fpsr(v: u64) {
        unsafe { core::arch::asm!("msr fpsr, {}", in(reg) v, options(nomem, nostack)) };
    }

    pub(crate) fn get_round() -> i32 {
        (fpcr() & RMODE) as i32
    }
    pub(crate) fn set_round(mode: i32) {
        set_fpcr((fpcr() & !RMODE) | mode as u64);
    }
    pub(crate) fn flags() -> i32 {
        (fpsr() as i32) & FE_ALL_EXCEPT
    }
    pub(crate) fn set_flags(mask: i32, value: i32) {
        let m = (mask & FE_ALL_EXCEPT) as u64;
        set_fpsr((fpsr() & !m) | (value as u64 & m));
    }
    pub(crate) fn save(env: &mut FEnv) {
        env.fpcr = fpcr() as u32;
        env.fpsr = fpsr() as u32;
    }
    pub(crate) fn restore(env: &FEnv) {
        set_fpcr(env.fpcr as u64);
        set_fpsr(env.fpsr as u64);
    }
    pub(crate) fn restore_default() {
        set_fpcr(0);
        set_fpsr(0);
    }
    /// Exception traps are already disabled; nothing to do beyond clearing the flags.
    pub(crate) fn mask_traps() {}
}

#[cfg(target_arch = "x86_64")]
mod arch {
    pub(crate) const FE_INVALID: i32 = 1;
    // Rust code here never raises it by name; C sees it in fenv.h.
    #[allow(dead_code)]
    pub(crate) const FE_DIVBYZERO: i32 = 4;
    pub(crate) const FE_OVERFLOW: i32 = 8;
    pub(crate) const FE_UNDERFLOW: i32 = 0x10;
    pub(crate) const FE_INEXACT: i32 = 0x20;
    /// x86's denormal-operand flag; not part of FE_ALL_EXCEPT.
    const FE_DENORMAL: i32 = 2;
    pub(crate) const FE_ALL_EXCEPT: i32 = 0x3d;
    pub(crate) const FE_TONEAREST: i32 = 0;
    pub(crate) const FE_DOWNWARD: i32 = 0x400;
    pub(crate) const FE_UPWARD: i32 = 0x800;
    pub(crate) const FE_TOWARDZERO: i32 = 0xc00;
    const RC: u16 = 0xc00;

    /// fexcept_t.
    pub type FExcept = u16;

    /// fenv_t: the 28-byte x87 environment image fnstenv stores, then MXCSR.
    #[repr(C)]
    pub struct FEnv {
        x87: [u16; 14],
        mxcsr: u32,
    }

    fn mxcsr() -> u32 {
        let mut v = 0u32;
        unsafe { core::arch::asm!("stmxcsr [{}]", in(reg) &mut v, options(nostack)) };
        v
    }
    fn set_mxcsr(v: u32) {
        unsafe { core::arch::asm!("ldmxcsr [{}]", in(reg) &v, options(nostack)) };
    }
    fn x87_control() -> u16 {
        let mut v = 0u16;
        unsafe { core::arch::asm!("fnstcw [{}]", in(reg) &mut v, options(nostack)) };
        v
    }
    fn set_x87_control(v: u16) {
        unsafe { core::arch::asm!("fldcw [{}]", in(reg) &v, options(nostack)) };
    }
    fn x87_status() -> u16 {
        let v: u16;
        unsafe { core::arch::asm!("fnstsw ax", out("ax") v, options(nomem, nostack)) };
        v
    }
    fn save_x87(env: &mut [u16; 14]) {
        // fnstenv masks every x87 exception as a side effect; fldenv undoes that.
        unsafe { core::arch::asm!("fnstenv [{0}]", "fldenv [{0}]", in(reg) env.as_mut_ptr(), options(nostack)) };
    }
    fn restore_x87(env: &[u16; 14]) {
        unsafe { core::arch::asm!("fldenv [{}]", in(reg) env.as_ptr(), options(nostack)) };
    }

    pub(crate) fn get_round() -> i32 {
        (x87_control() & RC) as i32
    }
    pub(crate) fn set_round(mode: i32) {
        set_x87_control((x87_control() & !RC) | mode as u16);
        // MXCSR.RC is bits 14:13, the same encoding three bits higher.
        set_mxcsr((mxcsr() & !(0x6000)) | ((mode as u32) << 3));
    }
    pub(crate) fn flags() -> i32 {
        ((x87_status() as u32 | mxcsr()) as i32) & FE_ALL_EXCEPT
    }
    pub(crate) fn set_flags(mask: i32, value: i32) {
        let m = mask & (FE_ALL_EXCEPT | FE_DENORMAL);
        let mut env = [0u16; 14];
        save_x87(&mut env);
        // The status word is the third 16-bit field of the image.
        env[2] &= !(m as u16);
        restore_x87(&env);
        set_mxcsr((mxcsr() & !(m as u32)) | (value & m) as u32);
    }
    pub(crate) fn save(env: &mut FEnv) {
        save_x87(&mut env.x87);
        env.mxcsr = mxcsr();
    }
    pub(crate) fn restore(env: &FEnv) {
        restore_x87(&env.x87);
        set_mxcsr(env.mxcsr);
    }
    pub(crate) fn restore_default() {
        unsafe { core::arch::asm!("fninit", options(nomem, nostack)) };
        set_mxcsr(0x1f80);
    }
    pub(crate) fn mask_traps() {
        set_x87_control(x87_control() | 0x3f);
        set_mxcsr(mxcsr() | 0x1f80);
    }
}

pub(crate) use arch::*;

/// FE_DFL_ENV is ((const fenv_t *)-1).
fn is_default(env: *const FEnv) -> bool {
    env as usize == usize::MAX
}

/// Sets the given exception flags.
pub(crate) fn raise(excepts: i32) {
    set_flags(excepts, excepts);
}

pub(crate) fn valid_round(mode: i32) -> bool {
    matches!(mode, FE_TONEAREST | FE_UPWARD | FE_DOWNWARD | FE_TOWARDZERO)
}

#[no_mangle]
pub extern "C" fn feclearexcept(excepts: i32) -> i32 {
    set_flags(excepts, 0);
    0
}

#[no_mangle]
pub extern "C" fn feraiseexcept(excepts: i32) -> i32 {
    raise(excepts);
    0
}

#[no_mangle]
pub extern "C" fn fetestexcept(excepts: i32) -> i32 {
    flags() & excepts & FE_ALL_EXCEPT
}

#[no_mangle]
pub unsafe extern "C" fn fegetexceptflag(flagp: *mut FExcept, excepts: i32) -> i32 {
    *flagp = (flags() & excepts & FE_ALL_EXCEPT) as FExcept;
    0
}

#[no_mangle]
pub unsafe extern "C" fn fesetexceptflag(flagp: *const FExcept, excepts: i32) -> i32 {
    set_flags(excepts, *flagp as i32);
    0
}

#[no_mangle]
pub extern "C" fn fegetround() -> i32 {
    get_round()
}

#[no_mangle]
pub extern "C" fn fesetround(mode: i32) -> i32 {
    if !valid_round(mode) {
        return -1;
    }
    set_round(mode);
    0
}

#[no_mangle]
pub unsafe extern "C" fn fegetenv(env: *mut FEnv) -> i32 {
    save(&mut *env);
    0
}

#[no_mangle]
pub unsafe extern "C" fn fesetenv(env: *const FEnv) -> i32 {
    if is_default(env) {
        restore_default();
    } else {
        restore(&*env);
    }
    0
}

/// Saves the environment, clears the flags and selects non-stop mode.
#[no_mangle]
pub unsafe extern "C" fn feholdexcept(env: *mut FEnv) -> i32 {
    save(&mut *env);
    set_flags(FE_ALL_EXCEPT, 0);
    mask_traps();
    0
}

/// Installs the saved environment, then raises the exceptions that were set before.
#[no_mangle]
pub unsafe extern "C" fn feupdateenv(env: *const FEnv) -> i32 {
    let pending = flags();
    fesetenv(env);
    raise(pending);
    0
}
