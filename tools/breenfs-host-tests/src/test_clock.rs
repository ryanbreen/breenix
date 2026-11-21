std::thread_local! {
    static UNIX_TIME: core::cell::Cell<u64> = const { core::cell::Cell::new(1) };
}

pub fn current_unix_time() -> u64 {
    UNIX_TIME.with(core::cell::Cell::get)
}

pub fn with_unix_time<T>(seconds: u64, f: impl FnOnce() -> T) -> T {
    struct Restore(u64);
    impl Drop for Restore {
        fn drop(&mut self) { UNIX_TIME.with(|clock| clock.set(self.0)); }
    }
    let _restore = Restore(UNIX_TIME.with(|clock| clock.replace(seconds)));
    f()
}
