#[cfg(test)]
mod inline_entry;

#[cfg(test)]
mod arch_impl {
    pub mod aarch64 {
        pub mod cpu {
            pub unsafe fn disable_interrupts() {}
        }
    }
}

#[cfg(test)]
mod task {
    pub mod scheduler {
        pub fn expire_signal_timers_arm64() {}
    }
}
