//! Restartable slot renames. Each completed phase is flushed before the next phase changes a source.
//! An interrupted rename is retried only while its source remains; the destination must exist otherwise.
pub trait Store {
    type Error;
    fn exists(&self, name: &str) -> bool;
    fn read(&self, name: &str, bytes: &mut [u8]) -> Result<usize, Self::Error>;
    fn write(&self, name: &str, bytes: &[u8]) -> Result<(), Self::Error>;
    fn delete(&self, name: &str) -> Result<(), Self::Error>;
    fn rename(&self, from: &str, to: &str) -> Result<(), Self::Error>;
    fn invalid(&self) -> Self::Error;
}

const JOURNAL: &str = "SLOT.TXN";
const MARKERS: [&str; 3] = ["SLOT0.OK", "SLOT1.OK", "SLOT2.OK"];

fn remove<S: Store>(store: &S, name: &str) -> Result<(), S::Error> {
    if store.exists(name) { store.delete(name)?; }
    Ok(())
}

fn move_pair<S: Store>(store: &S, from: [&str; 2], to: [&str; 2]) -> Result<(), S::Error> {
    for (source, target) in from.into_iter().zip(to) {
        if store.exists(source) {
            if store.exists(target) { return Err(store.invalid()); }
            store.rename(source, target)?;
        } else if !store.exists(target) {
            return Err(store.invalid());
        }
    }
    Ok(())
}

/// Finish the on-disk action before ordinary startup reads any slot notes.
pub fn recover<S: Store>(store: &S) -> Result<(), S::Error> {
    if !store.exists(JOURNAL) { return Ok(()); }
    let mut action = [0; 16];
    let len = store.read(JOURNAL, &mut action)?;
    let rollback = match &action[..len] {
        b"commit\n" => false,
        b"rollback\n" => true,
        _ => return Err(store.invalid()),
    };
    for (phase, marker) in MARKERS.iter().enumerate() {
        let mut done = [0; 8];
        if store.exists(marker) && store.read(marker, &mut done)? == 5 && &done[..5] == b"done\n" {
            continue;
        }
        match (rollback, phase) {
            (false, 0) => {
                remove(store, "PREVIOUS.IMG")?;
                remove(store, "PREVIOUS.TXT")?;
            }
            (false, 1) => {
                // The first install has no current pair; the journal remembers that by completing this phase
                // before any promotion. A partial pair resumes its remaining move on restart.
                if store.exists("CURRENT.IMG") || store.exists("CURRENT.TXT") || store.exists("PREVIOUS.IMG") {
                    move_pair(store, ["CURRENT.IMG", "CURRENT.TXT"], ["PREVIOUS.IMG", "PREVIOUS.TXT"])?;
                }
            }
            (false, 2) => move_pair(store, ["NEXT.IMG", "NEXT.TXT"], ["CURRENT.IMG", "CURRENT.TXT"] )?,
            (true, 0) => move_pair(store, ["CURRENT.IMG", "CURRENT.TXT"], ["SWAP.IMG", "SWAP.TXT"] )?,
            (true, 1) => move_pair(store, ["PREVIOUS.IMG", "PREVIOUS.TXT"], ["CURRENT.IMG", "CURRENT.TXT"] )?,
            (true, 2) => move_pair(store, ["SWAP.IMG", "SWAP.TXT"], ["PREVIOUS.IMG", "PREVIOUS.TXT"] )?,
            _ => unreachable!(),
        }
        store.write(marker, b"done\n")?;
    }
    // Remove the intent first: leftover completion markers do not identify an active action.
    store.delete(JOURNAL)?;
    Ok(())
}

/// Publish an immutable action only after its temporary file has passed write readback.
pub fn begin<S: Store>(store: &S, rollback: bool) -> Result<(), S::Error> {
    recover(store)?;
    for marker in MARKERS { remove(store, marker)?; }
    remove(store, "SLOT.NEW")?;
    store.write("SLOT.NEW", if rollback { b"rollback\n" } else { b"commit\n" })?;
    store.rename("SLOT.NEW", JOURNAL)?;
    recover(store)
}
