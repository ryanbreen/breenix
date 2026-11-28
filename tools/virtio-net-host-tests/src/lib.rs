//! Host checks of the ARM64 virtio-net MMIO driver. `drivers::virtio` mounts the
//! kernel's own driver files unchanged; the modules beside it stand in for the
//! kernel services those files call. Register accesses reach `device`, a fake
//! virtio-mmio network device that keeps its registers and reads and writes
//! the driver's rings in host memory, as a device would by DMA. The direct map
//! offset is 0 here, so a ring's "physical" address is its host address.
//!
//! Each file in `tests/` is one scenario in its own process: the driver keeps
//! its device in statics and is initialised once.

pub mod device;
pub mod support;

/// The kernel's `serial_println!`, discarded.
#[macro_export]
macro_rules! serial_println {
    ($($arg:tt)*) => {{
        let _ = format_args!($($arg)*);
    }};
}

pub mod drivers;

pub mod arch_impl {
    pub mod aarch64 {
        /// Device-register accesses go to the fake device.
        pub mod mmio {
            pub unsafe fn read8(address: *const u8) -> u8 {
                crate::device::read8(address as u64)
            }
            pub unsafe fn read32(address: *const u32) -> u32 {
                crate::device::read32(address as u64)
            }
            pub unsafe fn write32(address: *mut u32, value: u32) {
                crate::device::write32(address as u64, value)
            }
        }
        pub mod gic {
            pub struct Gicv2;
            impl crate::arch_impl::traits::InterruptController for Gicv2 {
                fn enable_irq(irq: u8) {
                    crate::device::note_irq_enabled(irq);
                }
            }
        }
        pub mod smp {
            pub const MAX_CPUS: usize = 8;
            /// The CPUs firmware reports, as PSCI enumeration reports them on QEMU.
            pub fn reported_cpus() -> (u64, &'static str) {
                (crate::device::reported_cpus(), "psci")
            }
        }
    }
    pub mod traits {
        pub trait InterruptController {
            fn enable_irq(irq: u8);
        }
    }
}

pub mod memory {
    #[derive(Clone, Copy)]
    pub struct VirtAddr(u64);
    impl VirtAddr {
        pub fn as_u64(self) -> u64 {
            self.0
        }
    }
    /// The direct map is the identity here.
    pub fn physical_memory_offset() -> VirtAddr {
        VirtAddr(0)
    }

    pub mod frame_allocator {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct PhysAddr(u64);
        impl PhysAddr {
            pub fn as_u64(self) -> u64 {
                self.0
            }
        }
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct PhysFrame(u64);
        impl PhysFrame {
            pub fn start_address(self) -> PhysAddr {
                PhysAddr(self.0)
            }
        }

        /// `count` adjacent 4 KiB frames from the host heap, kept for the
        /// process, and filled with 0xA5 as freshly allocated frames hold old data.
        pub fn allocate_contiguous_frames(
            count: usize,
            out: &mut [Option<PhysFrame>],
        ) -> Option<PhysFrame> {
            if count == 0 || out.len() < count || !crate::device::allocation_allowed(count) {
                return None;
            }
            let layout = std::alloc::Layout::from_size_align(count * 4096, 4096).ok()?;
            // SAFETY: `count` is at least 1; the memory is kept for the process.
            let base = unsafe { std::alloc::alloc(layout) };
            if base.is_null() {
                return None;
            }
            unsafe { core::ptr::write_bytes(base, 0xA5, count * 4096) };
            for (i, slot) in out[..count].iter_mut().enumerate() {
                *slot = Some(PhysFrame(base as u64 + 4096 * i as u64));
            }
            Some(PhysFrame(base as u64))
        }
    }
}

pub mod irq_safe_mutex {
    /// No interrupts on the host: the kernel's lock is the spin lock it wraps.
    pub struct IrqSafeMutex<T>(spin::Mutex<T>);
    impl<T> IrqSafeMutex<T> {
        pub const fn new(value: T) -> Self {
            Self(spin::Mutex::new(value))
        }
        pub fn lock(&self) -> spin::MutexGuard<'_, T> {
            self.0.lock()
        }
        pub fn try_lock(&self) -> Option<spin::MutexGuard<'_, T>> {
            self.0.try_lock()
        }
    }
}

pub mod task {
    pub mod softirqd {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum SoftirqType {
            NetRx,
        }
        pub fn raise_softirq(kind: SoftirqType) {
            crate::device::note_softirq(kind);
        }
    }
}
