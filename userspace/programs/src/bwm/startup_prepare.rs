//! Prepare independent title coverage while validating the bare desktop.

pub fn during<R>(prepare: impl FnOnce() -> Result<(), String> + Send,
    check: impl FnOnce() -> R,
) -> (Result<(), String>, R) {
    std::thread::scope(|scope| {
        let pending = scope.spawn(prepare);
        let checked = check();
        let prepared = pending.join().unwrap_or_else(|_| Err("title preparation worker panicked".into()));
        (prepared, checked)
    })
}
