//! One immutable boot contract for the CPUs which can dispatch userspace.
use core::arch::{
    asm,
    x86_64::{__cpuid, __cpuid_count},
};
use spin::Once;
use x86_64::registers::control::{Cr0, Cr0Flags, Cr4, Cr4Flags};

use super::xstate_layout::{Backend, CpuState, Layout, AVX, AVX512, BASELINE};

struct Contract {
    layout: Layout,
    hwcap: [u64; 2],
    mxcsr: u32,
}

static CONTRACT: Once<Contract> = Once::new();

pub fn layout() -> &'static Layout {
    &CONTRACT
        .get()
        .expect("CPU state contract initialized")
        .layout
}

pub fn hwcap() -> [u64; 2] {
    CONTRACT
        .get()
        .expect("CPU state contract initialized")
        .hwcap
}

pub fn mxcsr_mask() -> u32 {
    CONTRACT
        .get()
        .expect("CPU state contract initialized")
        .mxcsr
}

fn xsetbv(mask: u64) {
    unsafe {
        asm!("xsetbv", in("ecx") 0u32, in("eax") mask as u32, in("edx") (mask >> 32) as u32,
            options(nostack, preserves_flags));
    }
}

fn clear_msr(msr: u32) {
    unsafe {
        asm!("wrmsr", in("ecx") msr, in("eax") 0u32, in("edx") 0u32,
            options(nostack, preserves_flags));
    }
}

#[repr(align(16))]
struct FxProbe([u8; 512]);

pub fn init(cpu: usize) -> Result<Backend, &'static str> {
    let max = unsafe { __cpuid(0) }.eax;
    let leaf1 = unsafe { __cpuid(1) };
    let (leaf7_b, leaf7_d) = if max >= 7 {
        let leaf7 = unsafe { __cpuid_count(7, 0) };
        (leaf7.ebx, leaf7.edx)
    } else {
        (0, 0)
    };
    const LEGACY_FLAGS: u32 = 1 | (7 << 24); // FPU, FXSR, SSE, SSE2.
    if leaf1.edx & LEGACY_FLAGS != LEGACY_FLAGS {
        return Err("CPU lacks baseline x87/SSE2 state");
    }
    let xsave = max >= 0xd && leaf1.ecx & (1 << 26) != 0;
    let (descriptors, controls) = if xsave {
        let user = unsafe { __cpuid_count(0xd, 0) };
        let misc = unsafe { __cpuid_count(0xd, 1) };
        let user_mask = u64::from(user.eax) | (u64::from(user.edx) << 32);
        let xss_mask = if misc.eax & 8 != 0 {
            u64::from(misc.ecx) | (u64::from(misc.edx) << 32)
        } else {
            0
        };
        let descriptors = CpuState::probe(user_mask, xss_mask, user.ecx, misc.eax, |bit| {
            let leaf = unsafe { __cpuid_count(0xd, bit) };
            (leaf.eax, leaf.ebx, leaf.ecx)
        })
        .ok_or("invalid CPUID xstate descriptors")?;
        (Some(descriptors), misc.eax)
    } else {
        (None, 0)
    };

    let published = CONTRACT.get();
    let layout = if let Some(contract) = published {
        if !descriptors
            .as_ref()
            .map_or(contract.layout.backend == Backend::Fxsave, |d| {
                d.supports(&contract.layout)
            })
        {
            return Err("CPU cannot use the boot xstate layout");
        }
        contract.layout
    } else if let Some(descriptors) = &descriptors {
        let (reported, _) = descriptors.reported();
        let mut enabled = BASELINE;
        if leaf1.ecx & (1 << 28) != 0 && reported & AVX != 0 && descriptors.avx_layout_known() {
            enabled |= AVX;
            if leaf7_b & (1 << 16) != 0
                && reported & AVX512 == AVX512
                && descriptors.avx512_layout_known()
            {
                enabled |= AVX512;
            }
        }
        descriptors
            .layout(enabled, 0, descriptors.best_backend())
            .ok_or("invalid xstate layout")?
    } else {
        Layout::legacy()
    };

    if layout.user & AVX != 0 && leaf1.ecx & (1 << 28) == 0
        || layout.user & AVX512 != 0 && leaf7_b & (1 << 16) == 0
    {
        return Err("CPU lacks the instruction group for the boot xstate layout");
    }

    let initial = super::fpu::InitialState::allocate(&layout)?;
    let use_xsave = layout.backend != Backend::Fxsave;
    unsafe {
        Cr0::update(|cr0| {
            cr0.remove(Cr0Flags::EMULATE_COPROCESSOR | Cr0Flags::TASK_SWITCHED);
            cr0.insert(Cr0Flags::MONITOR_COPROCESSOR | Cr0Flags::NUMERIC_ERROR);
        });
        Cr4::update(|cr4| {
            cr4.insert(Cr4Flags::OSFXSR | Cr4Flags::OSXMMEXCPT_ENABLE);
            cr4.remove(Cr4Flags::OSXSAVE);
            if use_xsave {
                cr4.insert(Cr4Flags::OSXSAVE);
            }
        });
    }
    if use_xsave {
        xsetbv(layout.user);
        if controls & 8 != 0 {
            clear_msr(0xda0); // IA32_XSS. No supervisor-state owner is installed.
        }
        if controls & (1 << 4) != 0 {
            clear_msr(0x1c4); // IA32_XFD. Every enabled component is eagerly backed.
        }
        let standard = unsafe { __cpuid_count(0xd, 0) }.ebx as usize;
        let compacted = unsafe { __cpuid_count(0xd, 1) }.ebx as usize;
        if !layout.check_reported(standard, compacted) {
            return Err("CPU xstate span disagrees with CPUID");
        }
    }

    let mut probe = FxProbe([0; 512]);
    unsafe {
        asm!("fxsave64 [{}]", in(reg) probe.0.as_mut_ptr(), options(nostack, preserves_flags));
    }
    let mxcsr = u32::from_le_bytes(probe.0[28..32].try_into().unwrap());
    let mxcsr = if mxcsr == 0 { 0xffbf } else { mxcsr };

    let osxsave = use_xsave && unsafe { __cpuid(1) }.ecx & (1 << 27) != 0;
    let hwcap = super::xstate_layout::capabilities(
        leaf1.edx,
        leaf1.ecx,
        leaf7_b,
        leaf7_d,
        osxsave,
        layout.user,
    );
    initial.restore(&layout);
    if let Some(contract) = published {
        if !hwcap
            .into_iter()
            .zip(contract.hwcap)
            .all(|(local, common)| local & common == common)
            || mxcsr != contract.mxcsr
        {
            return Err("CPU lacks the boot userspace capability contract");
        }
    } else {
        CONTRACT.call_once(|| Contract {
            layout,
            hwcap,
            mxcsr,
        });
    }

    if let Some(descriptors) = descriptors {
        let (user, supervisor) = descriptors.reported();
        let disabled = user & !layout.user;
        if disabled | supervisor != 0 {
            log::info!("[xstate] cpu={} blocked user={:#x} thread/supervisor={:#x}: MPX needs #BR, AMX needs frame backing and XFD policy, PKRU needs kernel access ordering; other components need reviewed ownership and restore policies", cpu, disabled, supervisor);
        }
    }
    Ok(layout.backend)
}
