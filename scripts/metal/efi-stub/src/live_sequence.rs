//! Installed boot manager's live trial and promotion sequence.
use crate::live_flash::{self, Action, Manifest, Token};
use crate::slot_transaction::{self, Store};
use crate::uefi::{Status, ACCESS_DENIED, NOT_FOUND, VOLUME_CORRUPTED};

const MARKER: &str = "LIVE.ID";
const TRIAL: &str = "TRY.IMG";
const NEXT: &str = "NEXT.IMG";
const CURRENT_NOTE: &str = "CURRENT.TXT";
const NEXT_NOTE: &str = "NEXT.TXT";

/// Guest staging reads, retained ESP images and the ESP volume barrier.
pub trait Media {
    type Image: AsRef<[u8]>;

    fn read_record(&mut self, name: &[u8], bytes: &mut [u8]) -> Result<Option<usize>, Status>;
    fn staged_image(&mut self, size: usize) -> Result<Self::Image, Status>;
    fn retained_image(&self, next: bool, size: usize) -> Result<Self::Image, Status>;
    fn flush(&self) -> Result<(), Status>;
}

pub enum Outcome<I> {
    Current,
    Rollback(Token),
    StaleBase(Token),
    Trial(Manifest, I),
    Promoted(Manifest),
}

pub fn process<M: Media>(
    store: &impl Store<Error = Status>,
    media: &mut M,
    try_new: bool,
) -> Result<Outcome<M::Image>, Status> {
    let mut attempted_text = [0; 256];
    let attempted = if store.exists(MARKER) {
        let length = store.read(MARKER, &mut attempted_text)?;
        Some(Manifest::parse(&attempted_text[..length]).ok_or(VOLUME_CORRUPTED)?)
    } else {
        None
    };
    let mut receipt = [0; 33];
    let acknowledged = match media.read_record(b"committed", &mut receipt)? {
        Some(33) if receipt[32] == b'\n' => {
            Some(live_flash::token(&receipt[..32]).ok_or(VOLUME_CORRUPTED)?)
        }
        Some(_) => return Err(VOLUME_CORRUPTED),
        None => None,
    };

    if !try_new {
        if let Some(m) = attempted.filter(|m| acknowledged == Some(m.token)) {
            let note = slot_note(&m);
            if !read_matches(store, CURRENT_NOTE, &note)? {
                promote(store, media, &m, &note)?;
            }
            if read_matches(store, CURRENT_NOTE, &note)? {
                return Ok(Outcome::Promoted(m));
            }
        }
        return Ok(Outcome::Current);
    }

    let mut text = [0; 255];
    let Some(length) = media.read_record(b"manifest", &mut text)? else {
        return Ok(Outcome::Current);
    };
    let text = &text[..length];
    let m = Manifest::parse(text).ok_or(VOLUME_CORRUPTED)?;
    if live_flash::choose(Some(m.token), attempted.map(|m| m.token), None, false) != Action::Try {
        return Ok(if acknowledged == Some(m.token) {
            Outcome::Current
        } else {
            Outcome::Rollback(m.token)
        });
    }

    let mut base_note = [0; 256];
    let base_len = store.read(CURRENT_NOTE, &mut base_note)?;
    if !m.matches_base(&base_note[..base_len]) {
        return Ok(Outcome::StaleBase(m.token));
    }
    if store.exists(NEXT) || store.exists(NEXT_NOTE) {
        return Err(ACCESS_DENIED);
    }

    let image = media.staged_image(m.size)?;
    if !m.verifies(image.as_ref()) {
        return Err(VOLUME_CORRUPTED);
    }
    store.write(TRIAL, image.as_ref())?;
    // The retained image must be durable before the attempt can be published.
    media.flush()?;
    publish(store, media, "LIVE.NEW", MARKER, text)?;
    Ok(Outcome::Trial(m, image))
}

/// Publish a complete record through an alternate name. No caller starts
/// a trial until the rename has reached the device.
fn publish(
    store: &impl Store<Error = Status>,
    media: &impl Media,
    temp: &str,
    name: &str,
    bytes: &[u8],
) -> Result<(), Status> {
    store.write(temp, bytes)?;
    media.flush()?;
    if store.exists(name) {
        store.delete(name)?;
    }
    store.rename(temp, name)?;
    media.flush()
}

fn read_matches(
    store: &impl Store<Error = Status>,
    name: &str,
    expected: &[u8],
) -> Result<bool, Status> {
    let mut actual = [0; 256];
    match store.read(name, &mut actual) {
        Ok(len) => Ok(&actual[..len] == expected),
        Err(NOT_FOUND) => Ok(false),
        Err(status) => Err(status),
    }
}

/// Hand a confirmed trial to the original slot journal. NEXT.IMG only
/// appears after both the complete trial image and NEXT.TXT are durable.
fn promote(
    store: &impl Store<Error = Status>,
    media: &impl Media,
    m: &Manifest,
    note: &[u8],
) -> Result<(), Status> {
    let next = store.exists(NEXT);
    let ours = read_matches(store, NEXT_NOTE, note)?;
    if (next && !ours) || (!next && !store.exists(TRIAL)) {
        return Ok(());
    }
    let mut base = [0; 256];
    let len = store.read(CURRENT_NOTE, &mut base)?;
    if !m.matches_base(&base[..len]) || (!ours && store.exists(NEXT_NOTE)) {
        return Ok(());
    }
    if !m.verifies(media.retained_image(next, m.size)?.as_ref()) {
        return Err(VOLUME_CORRUPTED);
    }

    if !ours {
        publish(store, media, "TRY.NEW", NEXT_NOTE, note)?;
    }
    if !next {
        store.rename(TRIAL, NEXT)?;
        media.flush()?;
    }
    slot_transaction::begin(store, false)?;
    media.flush()
}

fn slot_note(manifest: &Manifest) -> [u8; 94] {
    let mut note = *b"commit 0000000000000000000000000000000000000000\ninstalled-by 00000000000000000000000000000000\n";
    note[7..47].copy_from_slice(&manifest.commit);
    note[61..93].copy_from_slice(&manifest.token);
    note
}
