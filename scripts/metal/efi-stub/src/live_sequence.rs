//! Installed boot manager's live trial and promotion sequence.
use crate::live_flash::{self, Manifest, Token, MANIFEST_MAX};
use crate::slot_transaction::{RecordStore, Records, Store};
use crate::uefi::{Status, ACCESS_DENIED, NOT_FOUND, VOLUME_CORRUPTED};

/// Guest staging reads and read-only access to the candidate's boot intent.
pub trait Media {
    type Image: AsRef<[u8]>;

    fn read_record(&mut self, name: &[u8], bytes: &mut [u8]) -> Result<Option<usize>, Status>;
    fn staged_image(&mut self, size: usize) -> Result<Self::Image, Status>;
    fn root_flags(&mut self, name: &[u8]) -> Result<Option<u32>, Status>;
    fn trial_intent(&mut self, image: &mut Self::Image, bytes: &mut [u8]) -> Result<usize, Status>;
}

pub enum Outcome<I> {
    Current,
    Rollback(Token),
    StaleBase(Token),
    Trial(Manifest, I),
    Promoted(Manifest),
}

pub fn process<M: Media>(
    store: &impl RecordStore,
    records: &mut Records,
    media: &mut M,
    try_new: bool,
) -> Result<Outcome<M::Image>, Status> {
    let mut record = records.latest().copied().ok_or(NOT_FOUND)?;
    let attempted = Manifest::parse(record.attempt());
    let mut receipt = [0; 33];
    let acknowledged = match media.read_record(b"committed", &mut receipt)? {
        Some(33) if receipt[32] == b'\n' => {
            Some(live_flash::token(&receipt[..32]).ok_or(VOLUME_CORRUPTED)?)
        }
        Some(_) => return Err(VOLUME_CORRUPTED),
        None => None,
    };
    let foreign_next = store.files().exists("NEXT.IMG") || store.files().exists("NEXT.TXT");
    if !try_new {
        if let Some(m) = attempted.filter(|m| acknowledged == Some(m.token)) {
            let note = slot_note(&m);
            if record.current.is_some_and(|current| {
                current.note() == note && current.size == m.size && current.digest == m.digest
            }) {
                return Ok(Outcome::Promoted(m));
            }
            if !foreign_next
                && record.live_trial() == Some(m)
                && record
                    .current
                    .is_some_and(|current| m.matches_base(current.note()))
            {
                record.trial.unwrap().verified(store)?;
                record.promote_trial();
                records.publish(store, record)?;
                return Ok(Outcome::Promoted(m));
            }
        }
        return Ok(Outcome::Current);
    }

    let mut text = [0; MANIFEST_MAX];
    let Some(length) = media.read_record(b"manifest", &mut text)? else {
        return Ok(Outcome::Current);
    };
    let text = text.get(..length).ok_or(VOLUME_CORRUPTED)?;
    let manifest = Manifest::parse(text).ok_or(VOLUME_CORRUPTED)?;
    if attempted.is_some_and(|previous| previous.token == manifest.token) {
        if acknowledged == Some(manifest.token) {
            return Ok(Outcome::Current);
        }
        if record.live_trial().is_some() {
            record.trial = None;
            records.publish(store, record)?;
        }
        return Ok(Outcome::Rollback(manifest.token));
    }

    let current = record.current.ok_or(NOT_FOUND)?;
    if !manifest.matches_base(current.note()) {
        return Ok(Outcome::StaleBase(manifest.token));
    }
    if foreign_next || (record.trial.is_some() && record.live_trial().is_none()) {
        return Err(ACCESS_DENIED);
    }

    if let Some(root) = manifest.root() {
        match media.root_flags(root)? {
            Some(0) => (),
            Some(_) => return Err(ACCESS_DENIED),
            None => return Err(NOT_FOUND),
        }
    }
    let mut image = media.staged_image(manifest.size)?;
    if !manifest.verifies(image.as_ref()) {
        return Err(VOLUME_CORRUPTED);
    }
    if let Some(root) = manifest.root() {
        let mut intent = [0; 1024];
        let len = media.trial_intent(&mut image, &mut intent)?;
        if !intent_names(intent.get(..len).ok_or(VOLUME_CORRUPTED)?, &root[4..]) {
            return Err(VOLUME_CORRUPTED);
        }
    }
    let slot = records.write_image(
        store,
        image.as_ref(),
        manifest.digest,
        &slot_note(&manifest),
    )?;
    record.trial = Some(slot);
    record.set_attempt(text)?;
    records.publish_image(store, record, slot)?;
    Ok(Outcome::Trial(manifest, image))
}

fn intent_names(bytes: &[u8], userland: &[u8]) -> bool {
    let bytes = bytes.split(|&byte| byte == 0).next().unwrap_or_default();
    let mut lines = bytes.split(|&byte| byte == b'\n');
    if !matches!(lines.next(), Some(b"BREENIX-BOOT 2" | b"BREENIX-BOOT 3")) {
        return false;
    }
    let mut names = lines.filter_map(|line| line.strip_prefix(b"userland "));
    names.next() == Some(userland) && names.next().is_none()
}

pub(crate) fn slot_note(manifest: &Manifest) -> [u8; 94] {
    let mut note = *b"commit 0000000000000000000000000000000000000000\ninstalled-by 00000000000000000000000000000000\n";
    note[7..47].copy_from_slice(&manifest.commit);
    note[61..93].copy_from_slice(&manifest.token);
    note
}
