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

/// Retry unavailable presentations in production. A presented pixel-check
/// result is returned unchanged, including a mismatch.
pub fn recover<T, E>(mut result: Result<T, E>, strict: bool,
    mut retry: impl FnMut(E) -> Result<T, E>,
) -> Result<T, E> {
    loop {
        match result {
            Ok(value) => return Ok(value),
            Err(error) if strict => return Err(error),
            Err(error) => result = retry(error),
        }
    }
}
