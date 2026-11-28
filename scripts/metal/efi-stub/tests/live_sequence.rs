use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::breenfs_files::{Reader, BLOCK, NODE};
use crate::live_flash::{Manifest, Token};

#[path = "../src/breenfs_crc32c.rs"]
mod fixture_crc;
use crate::live_sequence::{process, Media, Outcome};
use crate::sha256::sha256;
use crate::slot_transaction::{self, RecordStore, Records, Store, RECORD_PATHS, RECORD_SIZE};
use crate::uefi::{Status, DEVICE_ERROR, ERROR, LOAD_ERROR, NOT_FOUND, VOLUME_CORRUPTED};

const VOLUME_FULL: Status = ERROR | 11;
const CURRENT: &[u8] = &disk(1);
const PREVIOUS: &[u8] = &disk(2);
const CANDIDATE: &[u8] = &disk(3);
const DAMAGED: &[u8] = &disk(4);
const BASE: &[u8] = b"known-good note";
const TOKEN: Token = [b'1'; 32];
const STALE: Token = [b'2'; 32];
const ACK: &[u8] = b"11111111111111111111111111111111\n";
const STALE_ACK: &[u8] = b"22222222222222222222222222222222\n";

const fn disk(tag: u8) -> [u8; 1024] {
    let mut bytes = [tag; 1024];
    let mut i = 0;
    while i < 8 {
        bytes[512 + i] = b"EFI PART"[i];
        i += 1;
    }
    bytes
}

fn hex(digest: [u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn manifest(token: Token) -> Vec<u8> {
    manifest_for_base(token, BASE)
}

fn manifest_for_base(token: Token, base: &[u8]) -> Vec<u8> {
    format!(
        "BREENIX-FLASH 1\n{}\n{}\n{}\n{}\n{}\n",
        std::str::from_utf8(&token).unwrap(),
        "a".repeat(40),
        CANDIDATE.len(),
        hex(sha256(CANDIDATE)),
        hex(sha256(base)),
    )
    .into_bytes()
}

type Files = BTreeMap<String, Arc<[u8]>>;

struct Volume {
    files: RefCell<Files>,
    pending_sector: RefCell<Option<(String, usize, Vec<u8>)>>,
    cut: Option<(usize, bool)>,
    steps: Cell<usize>,
    lost_power: Cell<bool>,
    rename_order: Option<bool>,
    image_budget: usize,
    root_flags: Option<u32>,
    intent: &'static [u8],
}

impl Volume {
    fn new(files: Files, cut: Option<(usize, bool)>) -> Self {
        Self {
            files: RefCell::new(files),
            pending_sector: RefCell::new(None),
            cut,
            steps: Cell::new(0),
            lost_power: Cell::new(false),
            rename_order: None,
            image_budget: usize::MAX,
            root_flags: None,
            intent: b"",
        }
    }

    fn change(&self, apply: impl FnOnce(&mut Files)) -> Result<(), Status> {
        if self.lost_power.get() {
            return Err(DEVICE_ERROR);
        }
        let step = self.steps.get();
        self.steps.set(step + 1);
        if self.cut == Some((step, false)) {
            self.lost_power.set(true);
            return Err(DEVICE_ERROR);
        }
        apply(&mut self.files.borrow_mut());
        if self.cut == Some((step, true)) {
            self.lost_power.set(true);
            return Err(DEVICE_ERROR);
        }
        Ok(())
    }

    fn snapshot(&self) -> Files {
        self.files.borrow().clone()
    }

    fn sectors(&self, name: &str, bytes: &[u8], hold_last_sector: bool) -> Result<(), Status> {
        for (sector, chunk) in bytes.chunks(512).enumerate() {
            let offset = sector * 512;
            self.change(|files| {
                if hold_last_sector && offset + chunk.len() == bytes.len() {
                    self.pending_sector
                        .replace(Some((name.into(), offset, chunk.to_vec())));
                } else {
                    write_sector(files, name, offset, chunk);
                }
            })?;
        }
        Ok(())
    }
}

fn write_sector(files: &mut Files, name: &str, offset: usize, bytes: &[u8]) {
    Arc::make_mut(files.get_mut(name).unwrap())[offset..offset + bytes.len()]
        .copy_from_slice(bytes);
}

impl Store for Volume {
    type Error = Status;

    fn exists(&self, name: &str) -> bool {
        self.files.borrow().contains_key(name)
    }

    fn read(&self, name: &str, bytes: &mut [u8]) -> Result<usize, Status> {
        let files = self.files.borrow();
        let data = files.get(name).ok_or(NOT_FOUND)?;
        bytes
            .get_mut(..data.len())
            .ok_or(VOLUME_CORRUPTED)?
            .copy_from_slice(data);
        Ok(data.len())
    }

    fn write(&self, name: &str, bytes: &[u8]) -> Result<(), Status> {
        self.change(|files| {
            files.insert(name.into(), vec![0; bytes.len()].into());
        })?;
        self.sectors(name, bytes, false)
    }

    fn delete(&self, name: &str) -> Result<(), Status> {
        self.change(|files| {
            files.remove(name).unwrap();
        })
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), Status> {
        let contents = self.files.borrow()[from].clone();
        let add = |files: &mut Files| assert!(files.insert(to.into(), contents).is_none());
        let remove = |files: &mut Files| {
            files.remove(from).unwrap();
        };
        match self.rename_order {
            Some(true) => self.change(add).and_then(|()| self.change(remove)),
            Some(false) => self.change(remove).and_then(|()| self.change(add)),
            None => self.change(|files| {
                add(files);
                remove(files);
            }),
        }
    }

    fn invalid(&self) -> Status {
        VOLUME_CORRUPTED
    }
}

impl RecordStore for Volume {
    type Image = Arc<[u8]>;

    fn files(&self) -> &impl Store<Error = Status> {
        self
    }

    fn read_image(&self, name: &str, expected: Option<usize>) -> Result<Self::Image, Status> {
        let files = self.files.borrow();
        let image = files.get(name).ok_or(NOT_FOUND)?;
        if expected.is_some_and(|size| size != image.len()) {
            return Err(VOLUME_CORRUPTED);
        }
        Ok(image.clone())
    }

    fn prepare_records(&self) -> Result<(), Status> {
        for name in RECORD_PATHS {
            if !self.exists(name) {
                self.write(name, &[0; RECORD_SIZE])?;
            }
        }
        self.flush()
    }

    fn overwrite(&self, name: &str, bytes: &[u8; RECORD_SIZE]) -> Result<(), Status> {
        assert_eq!(self.files.borrow()[name].len(), RECORD_SIZE);
        self.sectors(name, bytes, false)
    }

    fn write_new_image(&self, name: &str, bytes: &[u8]) -> Result<(), Status> {
        assert!(!self.exists(name), "immutable image name was reused");
        let used = self
            .files
            .borrow()
            .iter()
            .filter(|(name, _)| name.ends_with(".IMG"))
            .map(|(_, image)| image.len())
            .sum();
        let written = bytes.len().min(self.image_budget.saturating_sub(used));
        self.change(|files| {
            files.insert(name.into(), vec![0; written].into());
        })?;
        self.sectors(name, &bytes[..written], written == bytes.len())?;
        if written != bytes.len() {
            return Err(VOLUME_FULL);
        }
        Ok(())
    }

    fn truncate_image(&self, name: &str) -> Result<bool, Status> {
        if self.files.borrow()[name].is_empty() {
            return Ok(false);
        }
        self.change(|files| {
            *files.get_mut(name).unwrap() = [].into();
        })?;
        Ok(true)
    }

    fn visit_images(&self, visit: &mut dyn FnMut(&str)) -> Result<(), Status> {
        self.files.borrow().keys().for_each(|name| visit(name));
        Ok(())
    }

    fn flush(&self) -> Result<(), Status> {
        self.change(|files| {
            if let Some((name, offset, bytes)) = self.pending_sector.take() {
                write_sector(files, &name, offset, &bytes);
            }
        })
    }
}

struct Guest<'a> {
    esp: &'a Volume,
    manifest: Option<&'a [u8]>,
    image: &'a [u8],
    ack: Option<&'a [u8]>,
}

impl<'a> Media for Guest<'a> {
    type Image = &'a [u8];

    fn read_record(&mut self, name: &[u8], bytes: &mut [u8]) -> Result<Option<usize>, Status> {
        let record = match name {
            b"manifest" => self.manifest,
            b"committed" => self.ack,
            _ => panic!("unexpected guest record"),
        };
        let Some(record) = record else {
            return Ok(None);
        };
        bytes
            .get_mut(..record.len())
            .ok_or(VOLUME_CORRUPTED)?
            .copy_from_slice(record);
        Ok(Some(record.len()))
    }

    fn staged_image(&mut self, size: usize) -> Result<Self::Image, Status> {
        if self.image.len() != size {
            return Err(VOLUME_CORRUPTED);
        }
        Ok(self.image)
    }

    fn root_flags(&mut self, name: &[u8]) -> Result<Option<u32>, Status> {
        assert_eq!(name, ROOT.as_bytes());
        fixture_root_flags(name, self.esp.root_flags)
    }

    fn trial_intent(&mut self, _: &mut Self::Image, bytes: &mut [u8]) -> Result<usize, Status> {
        bytes[..self.esp.intent.len()].copy_from_slice(self.esp.intent);
        Ok(self.esp.intent.len())
    }
}

fn run<'a>(
    volume: &'a Volume,
    manifest: Option<&'a [u8]>,
    image: &'a [u8],
    ack: Option<&'a [u8]>,
    try_new: bool,
) -> Result<Outcome<&'a [u8]>, Status> {
    let mut records = Records::read(volume);
    let outcome = process(
        volume,
        &mut records,
        &mut Guest {
            esp: volume,
            manifest,
            image,
            ack,
        },
        try_new,
    )?;
    records.collect(volume)?;
    Ok(outcome)
}

fn initial() -> Files {
    [
        ("CURRENT.IMG".into(), CURRENT.into()),
        ("CURRENT.TXT".into(), BASE.into()),
        ("PREVIOUS.IMG".into(), PREVIOUS.into()),
        ("PREVIOUS.TXT".into(), b"older note".as_slice().into()),
        ("LIVE.ID".into(), manifest(STALE).into()),
        ("TRY.IMG".into(), b"leftover trial".as_slice().into()),
    ]
    .into()
}

fn imported() -> Files {
    let legacy = Volume::new(initial(), None);
    Records::read(&legacy).import(&legacy).unwrap();
    legacy.snapshot()
}

fn assert_current(volume: &Volume, image: &[u8], note: &[u8]) {
    let records = Records::read(volume);
    let current = records.latest().unwrap().current.unwrap();
    assert_eq!(current.verified(volume).unwrap().as_ref(), image);
    assert_eq!(current.note(), note);
}

fn assert_trial_ready(outcome: Outcome<&[u8]>, volume: &Volume, armed: &[u8]) {
    let Outcome::Trial(attempt, image) = outcome else {
        panic!("verified candidate must be ready to launch");
    };
    let records = Records::read(volume);
    let record = records.latest().unwrap();
    assert_eq!(attempt.token, TOKEN);
    assert_eq!(image, CANDIDATE);
    assert_eq!(
        record.trial.unwrap().verified(volume).unwrap().as_ref(),
        CANDIDATE
    );
    assert_eq!(record.attempt(), armed);
}

#[test]
fn manager_tries_once_and_requires_matching_acknowledgement() {
    let armed = manifest(TOKEN);
    let stage = Some(armed.as_slice());
    let clean = Volume::new(imported(), None);
    assert!(matches!(
        run(&clean, None, CANDIDATE, None, true),
        Ok(Outcome::Current)
    ));
    for (manifest, image) in [
        (Some(&armed[..armed.len() - 1]), CANDIDATE),
        (stage, &CANDIDATE[..CANDIDATE.len() - 1]),
        (stage, DAMAGED),
    ] {
        assert!(matches!(
            run(&clean, manifest, image, None, true),
            Err(VOLUME_CORRUPTED)
        ));
        assert_eq!(clean.steps.get(), 0);
    }
    assert_trial_ready(
        run(&clean, stage, CANDIDATE, None, true).unwrap(),
        &clean,
        &armed,
    );
    for ack in [None, Some(STALE_ACK)] {
        let reboot = Volume::new(clean.snapshot(), None);
        assert!(matches!(
            run(&reboot, stage, CANDIDATE, ack, false),
            Ok(Outcome::Current)
        ));
        assert!(matches!(
            run(&reboot, stage, CANDIDATE, ack, true),
            Ok(Outcome::Rollback(TOKEN))
        ));
        assert_current(&reboot, CURRENT, BASE);
        let record = Records::read(&reboot);
        assert!(record.latest().unwrap().trial.is_none());
        assert_eq!(record.latest().unwrap().attempt(), armed);
        let writes = reboot.steps.get();
        assert!(matches!(
            run(&reboot, stage, CANDIDATE, ack, true),
            Ok(Outcome::Rollback(TOKEN))
        ));
        assert_eq!(reboot.steps.get(), writes);
    }
}

#[test]
fn manager_resumes_acknowledged_promotion_after_power_loss() {
    let armed = manifest(TOKEN);
    let stage = Some(armed.as_slice());
    let trial = Volume::new(imported(), None);
    assert_trial_ready(
        run(&trial, stage, CANDIDATE, None, true).unwrap(),
        &trial,
        &armed,
    );
    let clean = Volume::new(trial.snapshot(), None);
    assert!(matches!(
        run(&clean, stage, CANDIDATE, Some(ACK), false),
        Ok(Outcome::Promoted(Manifest { token: TOKEN, .. }))
    ));
    let note = format!(
        "commit {}\ninstalled-by {}\n",
        "a".repeat(40),
        "1".repeat(32)
    );
    for step in 0..clean.steps.get() {
        for after in [false, true] {
            let volume = Volume::new(trial.snapshot(), Some((step, after)));
            assert!(matches!(
                run(&volume, stage, CANDIDATE, Some(ACK), false),
                Err(DEVICE_ERROR)
            ));
            let reboot = Volume::new(volume.snapshot(), None);
            assert!(
                matches!(
                    run(&reboot, stage, CANDIDATE, Some(ACK), false),
                    Ok(Outcome::Promoted(Manifest { token: TOKEN, .. }))
                ),
                "step {step}, after={after}"
            );
            assert!(matches!(
                run(&reboot, stage, CANDIDATE, Some(ACK), true),
                Ok(Outcome::Current)
            ));
            assert_current(&reboot, CANDIDATE, note.as_bytes());
            let records = Records::read(&reboot);
            let previous = records.latest().unwrap().previous.unwrap();
            assert_eq!(previous.verified(&reboot).unwrap().as_ref(), CURRENT);
            assert_eq!(previous.note(), BASE);
            for name in ["NEXT.IMG", "NEXT.TXT", "SLOT.TXN"] {
                assert!(!reboot.exists(name), "{name}, step {step}, after={after}");
            }
        }
    }
}

fn startup(volume: &Volume) -> Result<([u8; 32], Option<[u8; 32]>), Status> {
    let mut records = Records::read(volume);
    let mut verified = Vec::new();
    let result = if records.import(volume).is_ok() {
        records.collect(volume).unwrap();
        records.boot(volume, false, |_, _, image| {
            verified.push(sha256(&image));
            LOAD_ERROR
        })
    } else {
        slot_transaction::boot_legacy(volume, |_, image| {
            verified.push(sha256(&image));
            LOAD_ERROR
        })
    };
    Ok((*verified.first().ok_or(result)?, verified.get(1).copied()))
}

#[derive(Clone, Copy, Debug)]
enum Transition {
    Import,
    Try,
    Promote,
    Commit,
    Rollback,
    StageInstall,
    TrialRollback,
}

fn apply(volume: &Volume, transition: Transition, armed: &[u8]) -> Result<(), Status> {
    let mut records = Records::read(volume);
    match transition {
        Transition::Import => records.import(volume)?,
        Transition::Try => {
            run(volume, Some(armed), CANDIDATE, None, true)?;
        }
        Transition::Promote => {
            run(volume, Some(armed), CANDIDATE, Some(ACK), false)?;
        }
        Transition::Commit => {
            records.commit_install(volume)?;
        }
        Transition::Rollback => records.rollback(volume)?,
        Transition::StageInstall => records.stage_install(volume, CANDIDATE, b"candidate note")?,
        Transition::TrialRollback => {
            run(volume, Some(armed), CANDIDATE, None, true)?;
        }
    }
    Records::read(volume).collect(volume)
}

fn with_next(mut files: Files) -> Files {
    files.insert("NEXT.IMG".into(), CANDIDATE.into());
    files.insert("NEXT.TXT".into(), b"candidate note".as_slice().into());
    files
}

#[test]
fn t_c1_each_esp_write_preserves_a_pair() {
    let armed = manifest(TOKEN);
    let imported = imported();
    let tried = Volume::new(imported.clone(), None);
    apply(&tried, Transition::Try, &armed).unwrap();
    let installed = Volume::new(imported.clone(), None);
    Records::read(&installed)
        .stage_install(&installed, CANDIDATE, b"candidate note")
        .unwrap();
    let old = (sha256(CURRENT), Some(sha256(PREVIOUS)));
    let promoted = (sha256(CANDIDATE), Some(sha256(CURRENT)));
    let swapped = (sha256(PREVIOUS), Some(sha256(CURRENT)));
    for (transition, source, expected) in [
        (Transition::Import, initial(), old),
        (Transition::Try, imported.clone(), old),
        (Transition::Promote, tried.snapshot(), promoted),
        (Transition::Commit, installed.snapshot(), promoted),
        (Transition::Commit, with_next(imported.clone()), promoted),
        (Transition::Rollback, imported.clone(), swapped),
        (Transition::StageInstall, imported.clone(), old),
        (Transition::TrialRollback, tried.snapshot(), old),
    ] {
        for both_first in [true, false] {
            let mut complete = Volume::new(source.clone(), None);
            complete.rename_order = Some(both_first);
            apply(&complete, transition, &armed).unwrap();
            assert_eq!(startup(&complete), Ok(expected), "{transition:?}");
            assert!(complete.steps.get() > 0);
            for step in 0..complete.steps.get() {
                for after in [false, true] {
                    let mut cut = Volume::new(source.clone(), Some((step, after)));
                    cut.rename_order = Some(both_first);
                    let _ = apply(&cut, transition, &armed);
                    assert!(
                        cut.lost_power.get(),
                        "{transition:?}, write {step}, after={after}"
                    );
                    let reboot = Volume::new(cut.snapshot(), None);
                    let pair = startup(&reboot).unwrap_or_else(|error| {
                        panic!("{transition:?}, write {step}, after={after}: {error:#x}")
                    });
                    assert!(
                        pair == old || pair == expected,
                        "{transition:?}, write {step}, after={after}"
                    );
                    let records = Records::read(&reboot);
                    let record = records.latest().unwrap();
                    if let Some(trial) = record.trial {
                        trial.verified(&reboot).unwrap();
                    }
                    if matches!(transition, Transition::Try) {
                        let outcome = run(&reboot, Some(&armed), CANDIDATE, None, true).unwrap();
                        if record.attempt() == armed {
                            assert!(matches!(outcome, Outcome::Rollback(TOKEN)));
                        } else {
                            assert_trial_ready(outcome, &reboot, &armed);
                        }
                    }
                }
            }
        }
    }

    // An older record preserves a third boot candidate. Reject a same-size
    // corrupt current and a missing previous before offering that candidate.
    let volume = Volume::new(tried.snapshot(), None);
    apply(&volume, Transition::Promote, &armed).unwrap();
    let records = Records::read(&volume);
    let mut damaged = volume.snapshot();
    damaged.insert(
        records
            .latest()
            .unwrap()
            .current
            .unwrap()
            .name
            .as_str()
            .into(),
        DAMAGED.into(),
    );
    damaged.remove("CURRENT.IMG");
    assert_eq!(
        startup(&Volume::new(damaged, None)),
        Ok((sha256(PREVIOUS), None))
    );

    // GC keeps older references, removes images dropped by both records, and
    // does not recycle a name left by an interrupted image creation.
    let volume = Volume::new(imported, None);
    let mut records = Records::read(&volume);
    volume.write_new_image("K0000010.IMG", CANDIDATE).unwrap();
    volume.flush().unwrap();
    records
        .stage_install(&volume, CANDIDATE, b"install one")
        .unwrap();
    let first = records.latest().unwrap().trial.unwrap().name;
    assert_eq!(first.as_str(), "K0000011.IMG");
    records
        .stage_install(&volume, CANDIDATE, b"install two")
        .unwrap();
    records.collect(&volume).unwrap();
    assert!(volume.exists(first.as_str()));
    assert!(!volume.exists("K0000010.IMG"));
    records
        .stage_install(&volume, CANDIDATE, b"install three")
        .unwrap();
    records.collect(&volume).unwrap();
    assert!(!volume.exists(first.as_str()));
}

#[test]
fn esp_capacity_allows_repeated_live_flashes_after_import() {
    let mut volume = Volume::new(imported(), None);
    volume.image_budget = 4 * CANDIDATE.len();
    let mut base = BASE.to_vec();
    for digit in b'1'..=b'4' {
        let token = [digit; 32];
        let armed = manifest_for_base(token, &base);
        let mut ack = [digit; 33];
        ack[32] = b'\n';
        assert!(matches!(
            run(&volume, Some(&armed), CANDIDATE, None, true).unwrap(),
            Outcome::Trial(m, _) if m.token == token
        ));
        assert!(matches!(
            run(&volume, None, CANDIDATE, Some(&ack), false).unwrap(),
            Outcome::Promoted(m) if m.token == token
        ));
        let records = Records::read(&volume);
        for record in [records.latest(), records.older()].into_iter().flatten() {
            for image in [record.current, record.previous, record.trial]
                .into_iter()
                .flatten()
            {
                image.verified(&volume).unwrap();
            }
        }
        base = records.latest().unwrap().current.unwrap().note().to_vec();
    }
    for name in ["CURRENT.IMG", "CURRENT.TXT", "PREVIOUS.IMG", "PREVIOUS.TXT", "TRY.IMG"] {
        assert!(!volume.exists(name), "{name} outlived its slot reference");
    }
}

#[test]
fn esp_capacity_recovers_bytes_after_failed_or_interrupted_write() {
    let mut files = imported();
    files.remove("TRY.IMG");
    files.insert("RESCUE.IMG".into(), CANDIDATE[..512].into());
    let mut volume = Volume::new(files, None);
    volume.image_budget = 3 * CANDIDATE.len();
    let armed = manifest(TOKEN);
    assert!(matches!(
        run(&volume, Some(&armed), CANDIDATE, None, true),
        Err(VOLUME_FULL)
    ));
    let mut failed = volume.snapshot();
    failed.remove("RESCUE.IMG");
    let mut interrupted = failed.clone();
    interrupted.insert("K0000002.IMG".into(), CANDIDATE[..512].into());

    for files in [failed, interrupted] {
        let mut reboot = Volume::new(files, None);
        reboot.image_budget = volume.image_budget;
        assert_trial_ready(
            run(&reboot, Some(&armed), CANDIDATE, None, true).unwrap(),
            &reboot,
            &armed,
        );
        assert!(reboot.exists("K0000003.IMG"));
        assert!(!reboot.exists("K0000002.IMG"));
    }
    assert_eq!(volume.snapshot()["K0000002.IMG"].len(), 0);
}

#[test]
fn t_c2_failed_import_boots_without_writing() {
    for both in [true, false] {
        let mut files = with_next(initial());
        files.insert("SLOT.TXN".into(), b"commit\n".as_slice().into());
        files.insert("SLOT0.OK".into(), b"done\n".as_slice().into());
        if both {
            files.insert("PREVIOUS.IMG".into(), CURRENT.into());
        } else {
            files.remove("CURRENT.IMG");
            files.remove("PREVIOUS.IMG");
        }
        let volume = Volume::new(files.clone(), None);
        let expected = if both { CURRENT } else { CANDIDATE };
        assert_eq!(
            startup(&volume).map(|pair| pair.0),
            Ok(sha256(expected)),
            "both={both}"
        );
        assert_eq!(volume.snapshot(), files);
        assert_eq!(volume.steps.get(), 0);
    }
}

const ROOT: &str = "gen-20261127T123456Z-metal";
const INTENT: &[u8] = b"BREENIX-BOOT 3\ntarget x86-metal\nuserland 20261127T123456Z-metal\n";

fn host_manifest(digest: [u8; 32], size: usize) -> Vec<u8> {
    std::fs::create_dir_all("target").unwrap();
    let path = format!("target/t-c3-{}-{size}.manifest", std::process::id());
    let output = std::process::Command::new("python3")
        .args([
            "-B",
            "-c",
            r"
import sys
sys.path.insert(0, '../')
from metal_live_flash import encode_manifest
with open(sys.argv[1], 'wb') as manifest:
    manifest.write(encode_manifest('1' * 32, 'a' * 40, int(sys.argv[2]), *sys.argv[3:]))
",
        ])
        .arg(&path)
        .args([&size.to_string(), &hex(digest), &hex(sha256(BASE)), ROOT])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    bytes
}

// ROOT 30 lives in a separate leaf from ROOT 1, so the scan must cross
// a leaf boundary and a sparse ID range. Values follow breenfs-format.md.
fn fixture_root_flags(name: &[u8], flags: Option<u32>) -> Result<Option<u32>, Status> {
    const UUID: [u8; 16] = [3; 16];
    const ROOT_OBJECT: u64 = 0xffff_ffff_ffff_ff10;
    const GENERATION: u64 = 4;

    fn stamp(bytes: &mut [u8]) -> [u8; 4] {
        let crc = fixture_crc::checksum(&bytes[12..]).to_le_bytes();
        bytes[8..12].copy_from_slice(&crc);
        crc
    }

    fn node(block: u64, level: u8, count: u16) -> [u8; NODE] {
        let mut node = [0; NODE];
        node[..8].copy_from_slice(b"BREENFSN");
        node[12] = 2;
        node[13] = level;
        node[14..16].copy_from_slice(&count.to_le_bytes());
        node[16..32].copy_from_slice(&UUID);
        node[32..40].copy_from_slice(&block.to_le_bytes());
        node[40..48].copy_from_slice(&GENERATION.to_le_bytes());
        node
    }

    fn key(bytes: &mut [u8], id: u64) {
        bytes[..8].copy_from_slice(&ROOT_OBJECT.to_le_bytes());
        bytes[8] = 0x90;
        bytes[9..17].copy_from_slice(&id.to_le_bytes());
    }

    fn item(block: u64, id: u64, name: &[u8], flags: u32) -> [u8; NODE] {
        let mut bytes = node(block, 0, 1);
        key(&mut bytes[48..65], id);
        let length = 65 + name.len();
        let offset = NODE - length;
        bytes[65..67].copy_from_slice(&(offset as u16).to_le_bytes());
        bytes[67..69].copy_from_slice(&(length as u16).to_le_bytes());
        bytes[offset + 60..offset + 64].copy_from_slice(&flags.to_le_bytes());
        bytes[offset + 64] = name.len() as u8;
        bytes[offset + 65..].copy_from_slice(name);
        stamp(&mut bytes);
        bytes
    }

    let left = item(24, 1, b"default", 0);
    let right = item(
        28,
        30,
        if flags.is_some() {
            ROOT.as_bytes()
        } else {
            b"other"
        },
        flags.unwrap_or(0),
    );
    let mut space = node(20, 1, 2);
    for (index, (id, block, child)) in [(1, 24u64, &left), (30, 28, &right)]
        .into_iter()
        .enumerate()
    {
        let entry = &mut space[48 + 37 * index..48 + 37 * (index + 1)];
        key(entry, id);
        entry[17..25].copy_from_slice(&block.to_le_bytes());
        entry[25..33].copy_from_slice(&GENERATION.to_le_bytes());
        entry[33..37].copy_from_slice(&child[8..12]);
    }
    let mut superblock = [0; BLOCK];
    superblock[..8].copy_from_slice(b"BREENFSS");
    superblock[12..16].copy_from_slice(&1u32.to_le_bytes());
    superblock[16..32].copy_from_slice(&UUID);
    superblock[32..40].copy_from_slice(&GENERATION.to_le_bytes());
    superblock[44..48].copy_from_slice(&(BLOCK as u32).to_le_bytes());
    superblock[48..52].copy_from_slice(&(NODE as u32).to_le_bytes());
    superblock[56..64].copy_from_slice(&128u64.to_le_bytes());
    superblock[80..88].copy_from_slice(&1u64.to_le_bytes());
    superblock[88..96].copy_from_slice(&20u64.to_le_bytes());
    superblock[96..104].copy_from_slice(&GENERATION.to_le_bytes());
    superblock[104..108].copy_from_slice(&stamp(&mut space));
    superblock[108] = 1;
    stamp(&mut superblock);

    let mut scratch = [0; NODE];
    let mut reader = Reader::open(128, &mut scratch, |block, out| {
        let bytes = match block {
            16 => superblock.as_slice(),
            20 => space.as_slice(),
            24 => left.as_slice(),
            28 => right.as_slice(),
            _ => return Err(()),
        };
        out.copy_from_slice(bytes);
        Ok(())
    })
    .map_err(|_| VOLUME_CORRUPTED)?;
    reader.root_flags(name).map_err(|_| VOLUME_CORRUPTED)
}

#[test]
fn t_c3_host_version_two_manifest_round_trips_and_checks_the_pair() {
    let imported = imported();
    for (size, length) in [(10_000_384, 256), (536_870_912, 257)] {
        let mut data = vec![0; size];
        data[..CANDIDATE.len()].copy_from_slice(CANDIDATE);
        let armed = host_manifest(sha256(&data), size);
        assert_eq!(armed.len(), length);
        assert_eq!(Manifest::parse(&armed).map(|m| m.size), Some(size));
        let mut volume = Volume::new(imported.clone(), None);
        volume.root_flags = Some(0);
        volume.intent = INTENT;
        assert!(matches!(
            run(&volume, Some(&armed), &data, None, true),
            Ok(Outcome::Trial(..))
        ));
        let reboot = Volume::new(volume.snapshot(), None);
        assert_eq!(Records::read(&reboot).latest().unwrap().attempt(), armed);
        assert!(matches!(
            run(&reboot, Some(&armed), &[], Some(ACK), false),
            Ok(Outcome::Promoted(..))
        ));
        assert_eq!(Records::read(&reboot).latest().unwrap().attempt(), armed);
    }
    let armed = host_manifest(sha256(CANDIDATE), CANDIDATE.len());
    for (flags, intent, expected) in [
        (None, INTENT, NOT_FOUND),
        (Some(1), INTENT, crate::uefi::ACCESS_DENIED),
        (
            Some(0),
            b"BREENIX-BOOT 3\nuserland 20261127T123457Z-metal\n".as_slice(),
            VOLUME_CORRUPTED,
        ),
    ] {
        let mut volume = Volume::new(imported.clone(), None);
        volume.root_flags = flags;
        volume.intent = intent;
        assert!(
            matches!(run(&volume, Some(&armed), CANDIDATE, None, true), Err(status) if status == expected)
        );
        assert_eq!(volume.steps.get(), 0);
    }
    let mut volume = Volume::new(imported, None);
    volume.root_flags = Some(0);
    volume.intent = INTENT;
    let mut too_long = armed;
    too_long.resize(513, b'x');
    assert!(matches!(
        run(&volume, Some(&too_long), CANDIDATE, None, true),
        Err(VOLUME_CORRUPTED)
    ));
    assert_eq!(volume.steps.get(), 0);
    let old_manager_input = Volume::new(volume.snapshot(), None);
    let legacy = manifest(TOKEN);
    assert!(matches!(
        run(&old_manager_input, Some(&legacy), CANDIDATE, None, true),
        Ok(Outcome::Trial(..))
    ));
}
