//! Redundant slot records and immutable images on the EFI system partition.
//! The legacy rename journal is used during import and by pre-record installers.
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
    if store.exists(name) {
        store.delete(name)?;
    }
    Ok(())
}

fn move_pair<S: Store>(store: &S, from: [&str; 2], to: [&str; 2]) -> Result<(), S::Error> {
    for (source, target) in from.into_iter().zip(to) {
        if store.exists(source) {
            if store.exists(target) {
                return Err(store.invalid());
            }
            store.rename(source, target)?;
        } else if !store.exists(target) {
            return Err(store.invalid());
        }
    }
    Ok(())
}

/// Finish the on-disk action before ordinary startup reads any slot notes.
pub fn recover<S: Store>(store: &S) -> Result<(), S::Error> {
    if !store.exists(JOURNAL) {
        return Ok(());
    }
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
                if store.exists("CURRENT.IMG")
                    || store.exists("CURRENT.TXT")
                    || store.exists("PREVIOUS.IMG")
                {
                    move_pair(
                        store,
                        ["CURRENT.IMG", "CURRENT.TXT"],
                        ["PREVIOUS.IMG", "PREVIOUS.TXT"],
                    )?;
                }
            }
            (false, 2) => move_pair(
                store,
                ["NEXT.IMG", "NEXT.TXT"],
                ["CURRENT.IMG", "CURRENT.TXT"],
            )?,
            (true, 0) => move_pair(
                store,
                ["CURRENT.IMG", "CURRENT.TXT"],
                ["SWAP.IMG", "SWAP.TXT"],
            )?,
            (true, 1) => move_pair(
                store,
                ["PREVIOUS.IMG", "PREVIOUS.TXT"],
                ["CURRENT.IMG", "CURRENT.TXT"],
            )?,
            (true, 2) => move_pair(
                store,
                ["SWAP.IMG", "SWAP.TXT"],
                ["PREVIOUS.IMG", "PREVIOUS.TXT"],
            )?,
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
    for marker in MARKERS {
        remove(store, marker)?;
    }
    remove(store, "SLOT.NEW")?;
    store.write(
        "SLOT.NEW",
        if rollback { b"rollback\n" } else { b"commit\n" },
    )?;
    store.rename("SLOT.NEW", JOURNAL)?;
    recover(store)
}

use core::cell::Cell;

use crate::live_flash::{Manifest, MANIFEST_MAX, MAX_IMAGE};
use crate::sha256::sha256;
use crate::uefi::{Status, ACCESS_DENIED, BAD_BUFFER_SIZE, NOT_FOUND, VOLUME_CORRUPTED};

pub const RECORD_SIZE: usize = 4096;
pub const NOTE_MAX: usize = 256;
pub const RECORD_PATHS: [&str; 2] = [r"A\SLOTS.REC", r"B\SLOTS.REC"];
const MAGIC: &[u8; 16] = b"BREENIX-SLOTS 1\n";
const RECORD_DIGEST: usize = RECORD_SIZE - 32;
const IMAGE_SIZE: usize = 12 + 4 + 32 + 2 + NOTE_MAX;
const LEGACY_PAIRS: [&str; 8] = [
    "CURRENT.IMG",
    "CURRENT.TXT",
    "PREVIOUS.IMG",
    "PREVIOUS.TXT",
    "NEXT.IMG",
    "NEXT.TXT",
    "SWAP.IMG",
    "SWAP.TXT",
];
const LEGACY_BOOT: [&str; 5] = [
    "CURRENT.IMG",
    "PREVIOUS.IMG",
    "NEXT.IMG",
    "SWAP.IMG",
    "TRY.IMG",
];

/// Check the complete legacy replay before any of its FAT edits. In particular,
/// deleting a name in a torn rename can free the other name's cluster chain.
fn check_legacy_recovery(store: &impl Store<Error = Status>) -> Result<(), Status> {
    struct Preview<'a, S> {
        store: &'a S,
        present: Cell<[bool; LEGACY_PAIRS.len()]>,
    }

    impl<S: Store<Error = Status>> Preview<'_, S> {
        fn set(&self, name: &str, present: bool) {
            if let Some(index) = LEGACY_PAIRS.iter().position(|&entry| entry == name) {
                let mut files = self.present.get();
                files[index] = present;
                self.present.set(files);
            }
        }
    }

    impl<S: Store<Error = Status>> Store for Preview<'_, S> {
        type Error = Status;
        fn exists(&self, name: &str) -> bool {
            LEGACY_PAIRS
                .iter()
                .position(|&entry| entry == name)
                .map_or_else(
                    || self.store.exists(name),
                    |index| self.present.get()[index],
                )
        }
        fn read(&self, name: &str, bytes: &mut [u8]) -> Result<usize, Status> {
            self.store.read(name, bytes)
        }
        fn write(&self, name: &str, _: &[u8]) -> Result<(), Status> {
            self.set(name, true);
            Ok(())
        }
        fn delete(&self, name: &str) -> Result<(), Status> {
            self.set(name, false);
            Ok(())
        }
        fn rename(&self, from: &str, to: &str) -> Result<(), Status> {
            self.set(from, false);
            self.set(to, true);
            Ok(())
        }
        fn invalid(&self) -> Status {
            VOLUME_CORRUPTED
        }
    }

    recover(&Preview {
        store,
        present: Cell::new(LEGACY_PAIRS.map(|name| store.exists(name))),
    })
}

/// The record and immutable-image operations used by both the manager and installer.
pub trait RecordStore {
    type Image: AsRef<[u8]>;

    fn files(&self) -> &impl Store<Error = Status>;
    fn read_image(&self, name: &str, expected: Option<usize>) -> Result<Self::Image, Status>;
    fn prepare_records(&self) -> Result<(), Status>;
    fn overwrite(&self, name: &str, bytes: &[u8; RECORD_SIZE]) -> Result<(), Status>;
    fn write_new_image(&self, name: &str, bytes: &[u8]) -> Result<(), Status>;
    fn visit_images(&self, visit: &mut dyn FnMut(&str)) -> Result<(), Status>;
    fn flush(&self) -> Result<(), Status>;
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ImageName([u8; 12]);

impl ImageName {
    fn parse(name: &str) -> Option<Self> {
        if !matches!(name, "CURRENT.IMG" | "PREVIOUS.IMG" | "TRY.IMG")
            && image_sequence(name).is_none()
        {
            return None;
        }
        let mut bytes = [0; 12];
        bytes[..name.len()].copy_from_slice(name.as_bytes());
        Some(Self(bytes))
    }

    fn new(sequence: u64) -> Result<Self, Status> {
        if sequence > 9_999_999 {
            return Err(ACCESS_DENIED);
        }
        let mut name = *b"K0000000.IMG";
        let mut sequence = sequence;
        for digit in name[1..8].iter_mut().rev() {
            *digit += (sequence % 10) as u8;
            sequence /= 10;
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.0)
            .unwrap()
            .trim_end_matches('\0')
    }
}

fn image_sequence(name: &str) -> Option<u64> {
    let digits = name.strip_prefix('K')?.strip_suffix(".IMG")?;
    if digits.len() != 7 || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

#[derive(Clone, Copy)]
pub struct Image {
    pub name: ImageName,
    pub size: usize,
    pub digest: [u8; 32],
    note: [u8; NOTE_MAX],
    note_len: usize,
}

impl Image {
    fn new(name: ImageName, size: usize, digest: [u8; 32], note: &[u8]) -> Result<Self, Status> {
        if size == 0 || size > MAX_IMAGE || note.len() > NOTE_MAX {
            return Err(VOLUME_CORRUPTED);
        }
        let mut entry = Self {
            name,
            size,
            digest,
            note: [0; NOTE_MAX],
            note_len: note.len(),
        };
        entry.note[..note.len()].copy_from_slice(note);
        Ok(entry)
    }

    pub fn note(&self) -> &[u8] {
        &self.note[..self.note_len]
    }

    pub fn verified<S: RecordStore>(&self, store: &S) -> Result<S::Image, Status> {
        let image = store.read_image(self.name.as_str(), Some(self.size))?;
        let bytes = image.as_ref();
        if bytes.len() != self.size || !looks_like_disk(bytes) || sha256(bytes) != self.digest {
            return Err(VOLUME_CORRUPTED);
        }
        Ok(image)
    }

    fn encode(&self, bytes: &mut [u8]) {
        bytes[..12].copy_from_slice(&self.name.0);
        bytes[12..16].copy_from_slice(&(self.size as u32).to_le_bytes());
        bytes[16..48].copy_from_slice(&self.digest);
        bytes[48..50].copy_from_slice(&(self.note_len as u16).to_le_bytes());
        bytes[50..].copy_from_slice(&self.note);
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        let name = ImageName::parse(
            core::str::from_utf8(&bytes[..12])
                .ok()?
                .trim_end_matches('\0'),
        )?;
        let size = u32::from_le_bytes(bytes[12..16].try_into().ok()?) as usize;
        let digest = bytes[16..48].try_into().ok()?;
        let note_len = u16::from_le_bytes(bytes[48..50].try_into().ok()?) as usize;
        Self::new(name, size, digest, bytes.get(50..50 + note_len)?).ok()
    }
}

pub fn looks_like_disk(bytes: &[u8]) -> bool {
    // SAFETY: the slice describes readable image bytes.
    unsafe { crate::bootdisk::looks_like_disk(bytes.as_ptr(), bytes.len()) }
}

#[derive(Clone, Copy)]
pub struct Record {
    sequence: u64,
    pub current: Option<Image>,
    pub previous: Option<Image>,
    pub trial: Option<Image>,
    attempt: [u8; MANIFEST_MAX],
    attempt_len: usize,
}

impl Record {
    const EMPTY: Self = Self {
        sequence: 0,
        current: None,
        previous: None,
        trial: None,
        attempt: [0; MANIFEST_MAX],
        attempt_len: 0,
    };

    pub fn attempt(&self) -> &[u8] {
        &self.attempt[..self.attempt_len]
    }

    pub fn set_attempt(&mut self, text: &[u8]) -> Result<(), Status> {
        if text.len() > MANIFEST_MAX {
            return Err(VOLUME_CORRUPTED);
        }
        self.attempt = [0; MANIFEST_MAX];
        self.attempt[..text.len()].copy_from_slice(text);
        self.attempt_len = text.len();
        Ok(())
    }

    pub fn live_trial(&self) -> Option<Manifest> {
        let manifest = Manifest::parse(self.attempt())?;
        let entry = self.trial?;
        (entry.size == manifest.size
            && entry.digest == manifest.digest
            && entry.note() == crate::live_sequence::slot_note(&manifest))
        .then_some(manifest)
    }

    pub fn promote_trial(&mut self) {
        self.previous = self.current;
        self.current = self.trial.take();
    }

    fn names(&self, name: &str) -> bool {
        [self.current, self.previous, self.trial]
            .into_iter()
            .flatten()
            .any(|image| image.name.as_str() == name)
    }

    fn encode(&self) -> [u8; RECORD_SIZE] {
        let mut bytes = [0; RECORD_SIZE];
        bytes[..16].copy_from_slice(MAGIC);
        bytes[16..24].copy_from_slice(&self.sequence.to_le_bytes());
        bytes[24..26].copy_from_slice(&(self.attempt_len as u16).to_le_bytes());
        bytes[26..26 + MANIFEST_MAX].copy_from_slice(&self.attempt);
        let at = 26 + MANIFEST_MAX;
        for (index, image) in [self.current, self.previous, self.trial]
            .into_iter()
            .enumerate()
        {
            if let Some(image) = image {
                let start = at + index * IMAGE_SIZE;
                image.encode(&mut bytes[start..start + IMAGE_SIZE]);
            }
        }
        let digest = sha256(&bytes[..RECORD_DIGEST]);
        bytes[RECORD_DIGEST..].copy_from_slice(&digest);
        bytes
    }

    fn decode(bytes: &[u8; RECORD_SIZE]) -> Option<Self> {
        if &bytes[..16] != MAGIC || sha256(&bytes[..RECORD_DIGEST]) != bytes[RECORD_DIGEST..] {
            return None;
        }
        let sequence = u64::from_le_bytes(bytes[16..24].try_into().ok()?);
        if sequence == 0 {
            return None;
        }
        let len = u16::from_le_bytes(bytes[24..26].try_into().ok()?) as usize;
        let attempt = bytes.get(26..26 + len)?;
        if len != 0 {
            Manifest::parse(attempt)?;
        }
        let mut record = Self {
            sequence,
            ..Self::EMPTY
        };
        record.set_attempt(attempt).ok()?;
        let at = 26 + MANIFEST_MAX;
        for (index, slot) in [&mut record.current, &mut record.previous, &mut record.trial]
            .into_iter()
            .enumerate()
        {
            let start = at + index * IMAGE_SIZE;
            let raw = &bytes[start..start + IMAGE_SIZE];
            if raw[0] != 0 {
                let image = Image::decode(raw)?;
                if image_sequence(image.name.as_str()).is_some_and(|n| n > sequence) {
                    return None;
                }
                *slot = Some(image);
            }
        }
        Some(record)
    }
}

pub struct Records {
    sides: [Option<Record>; 2],
    newest: usize,
    unreadable: Option<Status>,
}

impl Records {
    pub fn read(store: &impl RecordStore) -> Self {
        let mut bytes = [0; RECORD_SIZE];
        let mut unreadable = None;
        let sides = RECORD_PATHS.map(|name| match store.files().read(name, &mut bytes) {
            Ok(RECORD_SIZE) => Record::decode(&bytes),
            Ok(_) | Err(NOT_FOUND | BAD_BUFFER_SIZE) => None,
            Err(status) => {
                unreadable = Some(status);
                None
            }
        });
        let newest = usize::from(sides[1].map(|r| r.sequence) > sides[0].map(|r| r.sequence));
        if let [Some(a), Some(b)] = sides {
            if a.sequence == b.sequence {
                unreadable = Some(VOLUME_CORRUPTED);
            }
        }
        Self {
            sides,
            newest,
            unreadable,
        }
    }

    pub fn latest(&self) -> Option<&Record> {
        self.sides[self.newest].as_ref()
    }

    pub fn older(&self) -> Option<&Record> {
        self.sides[1 - self.newest].as_ref()
    }

    pub fn check_readable(&self) -> Result<(), Status> {
        self.unreadable.map_or(Ok(()), Err)
    }

    fn next_sequence(&self) -> Result<u64, Status> {
        self.check_readable()?;
        self.latest()
            .map_or(0, |r| r.sequence)
            .checked_add(1)
            .ok_or(ACCESS_DENIED)
    }

    fn publish_at(
        &mut self,
        store: &impl RecordStore,
        mut record: Record,
        sequence: u64,
    ) -> Result<(), Status> {
        let side = if self.latest().is_some() {
            1 - self.newest
        } else {
            0
        };
        record.sequence = sequence;
        store.overwrite(RECORD_PATHS[side], &record.encode())?;
        store.flush()?;
        self.sides[side] = Some(record);
        self.newest = side;
        Ok(())
    }

    fn unused_sequence(&self, store: &impl RecordStore) -> Result<u64, Status> {
        let mut sequence = self.next_sequence()?;
        store.visit_images(&mut |name| {
            if let Some(used) = image_sequence(name) {
                sequence = sequence.max(used + 1);
            }
        })?;
        Ok(sequence)
    }

    pub fn publish(&mut self, store: &impl RecordStore, record: Record) -> Result<(), Status> {
        let sequence = self.unused_sequence(store)?;
        self.publish_at(store, record, sequence)
    }

    pub fn collect(&self, store: &impl RecordStore) -> Result<(), Status> {
        self.check_readable()?;
        let Some(record) = self.latest() else {
            return Ok(());
        };
        loop {
            let mut orphan = None;
            store.visit_images(&mut |name| {
                // A name from an interrupted creation is the allocation
                // watermark until a subsequent record has passed its sequence.
                if image_sequence(name).is_some_and(|n| n <= record.sequence)
                    && !self.sides.iter().flatten().any(|record| record.names(name))
                {
                    orphan = ImageName::parse(name);
                }
            })?;
            let Some(name) = orphan else { return Ok(()) };
            store.files().delete(name.as_str())?;
            store.flush()?;
        }
    }

    pub fn write_image(
        &self,
        store: &impl RecordStore,
        bytes: &[u8],
        digest: [u8; 32],
        note: &[u8],
    ) -> Result<Image, Status> {
        let name = ImageName::new(self.unused_sequence(store)?)?;
        // The caller publishes the image and its sequence together.
        let entry = Image::new(name, bytes.len(), digest, note)?;
        store.write_new_image(name.as_str(), bytes)?;
        store.flush()?;
        Ok(entry)
    }

    pub fn publish_image(
        &mut self,
        store: &impl RecordStore,
        record: Record,
        image: Image,
    ) -> Result<(), Status> {
        let sequence = image_sequence(image.name.as_str()).ok_or(VOLUME_CORRUPTED)?;
        if sequence < self.next_sequence()? {
            return Err(VOLUME_CORRUPTED);
        }
        self.publish_at(store, record, sequence)
    }

    pub fn stage_install(
        &mut self,
        store: &impl RecordStore,
        bytes: &[u8],
        note: &[u8],
    ) -> Result<(), Status> {
        if !looks_like_disk(bytes) {
            return Err(VOLUME_CORRUPTED);
        }
        let mut record = self.latest().copied().unwrap_or(Record::EMPTY);
        if self.latest().is_none() {
            self.next_sequence()?;
            store.prepare_records()?;
        }
        let image = self.write_image(store, bytes, sha256(bytes), note)?;
        record.trial = Some(image);
        self.publish_image(store, record, image)
    }

    pub fn commit_install(&mut self, store: &impl RecordStore) -> Result<bool, Status> {
        let mut record = self.latest().copied().ok_or(NOT_FOUND)?;
        let files = store.files();
        // An older recovery installer can still stage NEXT on an imported ESP.
        let changed = if files.exists("NEXT.IMG") {
            let image = store.read_image("NEXT.IMG", None)?;
            let mut note = [0; NOTE_MAX];
            let len = files.read("NEXT.TXT", &mut note)?;
            let bytes = image.as_ref();
            if !looks_like_disk(bytes) {
                return Err(VOLUME_CORRUPTED);
            }
            let digest = sha256(bytes);
            let same_current = record.current.is_some_and(|current| {
                current.size == bytes.len()
                    && current.digest == digest
                    && current.note() == &note[..len]
                    && current.verified(store).is_ok()
            });
            if !same_current {
                let next = self.write_image(store, bytes, digest, &note[..len])?;
                record.trial = Some(next);
                record.promote_trial();
                self.publish_image(store, record, next)?;
            }
            files.delete("NEXT.IMG")?;
            store.flush()?;
            true
        } else if let Some(next) = record.trial.filter(|_| record.live_trial().is_none()) {
            next.verified(store)?;
            record.promote_trial();
            self.publish(store, record)?;
            true
        } else {
            false
        };
        if files.exists("NEXT.TXT") {
            let mut note = [0; NOTE_MAX];
            let len = files.read("NEXT.TXT", &mut note)?;
            if record
                .current
                .is_some_and(|current| current.note() == &note[..len])
            {
                // Also completes cleanup after a cut following the unlink above.
                files.delete("NEXT.TXT")?;
                store.flush()?;
                return Ok(true);
            }
            return Err(VOLUME_CORRUPTED);
        }
        Ok(changed)
    }

    pub fn rollback(&mut self, store: &impl RecordStore) -> Result<(), Status> {
        let mut record = self.latest().copied().ok_or(NOT_FOUND)?;
        if record.previous.is_none() || record.current.is_none() {
            return Err(NOT_FOUND);
        }
        core::mem::swap(&mut record.current, &mut record.previous);
        if record.live_trial().is_some() {
            record.trial = None;
        }
        self.publish(store, record)
    }

    pub fn import(&mut self, store: &impl RecordStore) -> Result<(), Status> {
        if self.latest().is_some() {
            return Ok(());
        }
        self.next_sequence()?;
        let files = store.files();
        check_legacy_recovery(files)?;
        recover(files)?;
        let current = import_image(store, "CURRENT.IMG", "CURRENT.TXT")?;
        let previous = import_image(store, "PREVIOUS.IMG", "PREVIOUS.TXT")?;
        // A replacement installed before the first slot commit may only have NEXT.
        if current.is_none() && previous.is_none() && !files.exists("NEXT.IMG") {
            return Err(NOT_FOUND);
        }
        let mut record = Record {
            current,
            previous,
            ..Record::EMPTY
        };
        let mut text = [0; MANIFEST_MAX];
        if let Ok(len) = files.read("LIVE.ID", &mut text) {
            if let Some(manifest) = Manifest::parse(&text[..len]).filter(|m| m.root().is_none()) {
                record.set_attempt(&text[..len])?;
                if let Ok(image) = store.read_image("TRY.IMG", Some(manifest.size)) {
                    if manifest.verifies(image.as_ref()) && looks_like_disk(image.as_ref()) {
                        record.trial = Some(Image::new(
                            ImageName::parse("TRY.IMG").unwrap(),
                            manifest.size,
                            manifest.digest,
                            &crate::live_sequence::slot_note(&manifest),
                        )?);
                    }
                }
            }
        }
        store.prepare_records()?;
        self.publish(store, record)
    }

    /// Boot from the selected record, then its other slot and both older slots.
    /// The callback only returns if the loader could not take over.
    pub fn boot<S: RecordStore>(
        &self,
        store: &S,
        previous_first: bool,
        mut launch: impl FnMut(&Image, bool, S::Image) -> Status,
    ) -> Status {
        let mut seen = [None; 4];
        let mut count = 0;
        let mut status = NOT_FOUND;
        for (index, record) in [self.latest(), self.older()].into_iter().enumerate() {
            let Some(record) = record else { continue };
            let mut slots = [(record.current, false), (record.previous, true)];
            if previous_first && index == 0 {
                slots.swap(0, 1);
            }
            for (image, previous) in slots {
                let Some(image) = image else { continue };
                // A name identifies immutable bytes, but a damaged entry could
                // carry the wrong digest. Deduplicate only identical references.
                let reference = (image.name, image.size, image.digest);
                if seen[..count].contains(&Some(reference)) {
                    continue;
                }
                seen[count] = Some(reference);
                count += 1;
                status = match image.verified(store) {
                    Ok(pages) => launch(&image, previous, pages),
                    Err(error) => error,
                };
            }
        }
        status
    }
}

fn import_image(store: &impl RecordStore, name: &str, note: &str) -> Result<Option<Image>, Status> {
    let image = match store.read_image(name, None) {
        Ok(image) => image,
        Err(NOT_FOUND) => return Ok(None),
        Err(status) => return Err(status),
    };
    let bytes = image.as_ref();
    let mut text = [0; NOTE_MAX];
    let len = match store.files().read(note, &mut text) {
        Ok(len) => len,
        Err(NOT_FOUND) => 0,
        Err(status) => return Err(status),
    };
    Ok(Some(Image::new(
        ImageName::parse(name).ok_or(VOLUME_CORRUPTED)?,
        bytes.len(),
        sha256(bytes),
        &text[..len],
    )?))
}

/// This path takes no write-capable operation on the legacy volume.
pub fn boot_legacy<S: RecordStore>(
    store: &S,
    mut launch: impl FnMut(&str, S::Image) -> Status,
) -> Status {
    let mut status = NOT_FOUND;
    for name in LEGACY_BOOT {
        match store.read_image(name, None) {
            Ok(image) if looks_like_disk(image.as_ref()) => status = launch(name, image),
            Ok(_) => status = VOLUME_CORRUPTED,
            Err(NOT_FOUND) => (),
            Err(error) => status = error,
        }
    }
    status
}
