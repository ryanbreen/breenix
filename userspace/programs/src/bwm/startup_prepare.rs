//! Prepare independent title coverage while validating the bare desktop.

/// Prepare static chrome, the clock alphabet and the titles already registered.
/// Later title discovery can add newcomers without a second cold restart batch.
pub fn coverage<'a>(clock: &'a [u8], titles: &[&'a [u8]]) -> Vec<&'a [u8]> {
    let mut texts: Vec<&[u8]> = vec![b"Breenix", clock, b"0123456789: ET", b"x-"];
    texts.extend_from_slice(titles);
    texts
}

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

/// Retry unavailable presentations at most twice in production. Persistent
/// refusal must return to the caller instead of recreating GPU targets forever.
/// A presented pixel-check result is returned unchanged, including a mismatch.
pub fn recover<T, E>(mut result: Result<T, E>, strict: bool,
    mut retry: impl FnMut(E) -> Result<T, E>,
) -> Result<T, E> {
    let mut remaining = if strict { 0 } else { 2 };
    loop {
        match result {
            Ok(value) => return Ok(value),
            Err(error) if remaining == 0 => return Err(error),
            Err(error) => {
                remaining -= 1;
                result = retry(error);
            }
        }
    }
}
