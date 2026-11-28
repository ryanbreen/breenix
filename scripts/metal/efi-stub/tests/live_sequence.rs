use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use crate::live_flash::{Manifest, Token};
use crate::live_sequence::{process, Media, Outcome};
use crate::sha256::sha256;
use crate::slot_transaction::{self, Store};
use crate::uefi::{Status, DEVICE_ERROR, NOT_FOUND, VOLUME_CORRUPTED};

const CURRENT: &[u8] = &disk(1);
const PREVIOUS: &[u8] = &disk(2);
const CANDIDATE: &[u8] = &disk(3);
const DAMAGED: &[u8] = &disk(4);

const fn disk(tag: u8) -> [u8; 1024] {
    let mut bytes = [tag; 1024];
    let mut i = 0;
    while i < 8 {
        bytes[512 + i] = b"EFI PART"[i];
        i += 1;
    }
    bytes
}
const BASE: &[u8] = b"known-good note";
const TOKEN: Token = [b'1'; 32];
const STALE: Token = [b'2'; 32];
const ACK: &[u8] = b"11111111111111111111111111111111\n";
const STALE_ACK: &[u8] = b"22222222222222222222222222222222\n";

fn manifest(token: Token) -> Vec<u8> {
    fn hex(digest: [u8; 32]) -> String {
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }
    format!(
        "BREENIX-FLASH 1\n{}\n{}\n{}\n{}\n{}\n",
        std::str::from_utf8(&token).unwrap(),
        "a".repeat(40),
        CANDIDATE.len(),
        hex(sha256(CANDIDATE)),
        hex(sha256(BASE)),
    )
    .into_bytes()
}

type Files = BTreeMap<String, Vec<u8>>;

struct Volume {
    files: RefCell<Files>,
    // A crash can retain just a prefix of the large image until the volume flush.
    pending_image: RefCell<Option<Vec<u8>>>,
    // The rename may also still be in the device cache until that flush.
    pending_marker: RefCell<Option<Vec<u8>>>,
    cut: Option<(usize, bool)>,
    steps: Cell<usize>,
    rename_order: Option<bool>,
    root_flags: Option<u32>,
    intent: &'static [u8],
}

impl Volume {
    fn new(files: Files, cut: Option<(usize, bool)>) -> Self {
        Self {
            files: RefCell::new(files),
            pending_image: RefCell::new(None),
            pending_marker: RefCell::new(None),
            cut,
            steps: Cell::new(0),
            rename_order: None,
            root_flags: None,
            intent: b"",
        }
    }

    fn change(&self, apply: impl FnOnce(&mut Files)) -> Result<(), Status> {
        let step = self.steps.get();
        self.steps.set(step + 1);
        if self.cut == Some((step, false)) {
            return Err(DEVICE_ERROR);
        }
        apply(&mut self.files.borrow_mut());
        if self.cut == Some((step, true)) {
            return Err(DEVICE_ERROR);
        }
        Ok(())
    }

    fn snapshot(&self) -> Files {
        self.files.borrow().clone()
    }
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
            let durable = if name == "TRY.IMG" {
                self.pending_image.replace(Some(bytes.to_vec()));
                &bytes[..bytes.len() / 2]
            } else {
                bytes
            };
            files.insert(name.into(), durable.to_vec());
        })
    }

    fn delete(&self, name: &str) -> Result<(), Status> {
        self.change(|files| {
            files.remove(name).unwrap();
        })
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), Status> {
        if let Some(both_first) = self.rename_order {
            let contents = self.files.borrow()[from].clone();
            let add = |files: &mut Files| assert!(files.insert(to.into(), contents).is_none());
            let remove = |files: &mut Files| { files.remove(from).unwrap(); };
            return if both_first {
                self.change(add).and_then(|()| self.change(remove))
            } else {
                self.change(remove).and_then(|()| self.change(add))
            };
        }
        self.change(|files| {
            let contents = files.remove(from).unwrap();
            assert!(!files.contains_key(to));
            if to == "LIVE.ID" {
                self.pending_marker.replace(Some(contents));
            } else {
                files.insert(to.into(), contents);
            }
        })
    }

    fn invalid(&self) -> Status {
        VOLUME_CORRUPTED
    }
}

struct Guest<'a> {
    esp: &'a Volume,
    manifest: Option<&'a [u8]>,
    image: &'a [u8],
    ack: Option<&'a [u8]>,
}

impl Media for Guest<'_> {
    type Image = Vec<u8>;

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
        Ok(self.image.to_vec())
    }

    fn retained_image(&self, next: bool, size: usize) -> Result<Self::Image, Status> {
        let name = if next { "NEXT.IMG" } else { "TRY.IMG" };
        let image = self
            .esp
            .files
            .borrow()
            .get(name)
            .cloned()
            .ok_or(NOT_FOUND)?;
        if image.len() != size {
            return Err(VOLUME_CORRUPTED);
        }
        Ok(image)
    }

    fn flush(&self) -> Result<(), Status> {
        self.esp.change(|files| {
            if let Some(image) = self.esp.pending_image.take() {
                files.insert("TRY.IMG".into(), image);
            }
            if let Some(marker) = self.esp.pending_marker.take() {
                files.insert("LIVE.ID".into(), marker);
            }
        })
    }
}

fn run(
    volume: &Volume,
    manifest: Option<&[u8]>,
    image: &[u8],
    ack: Option<&[u8]>,
    try_new: bool,
) -> Result<Outcome<Vec<u8>>, Status> {
    process(
        volume,
        &mut Guest {
            esp: volume,
            manifest,
            image,
            ack,
        },
        try_new,
    )
}

fn initial() -> Files {
    [
        ("CURRENT.IMG".into(), CURRENT.to_vec()),
        ("CURRENT.TXT".into(), BASE.to_vec()),
        ("PREVIOUS.IMG".into(), PREVIOUS.to_vec()),
        ("PREVIOUS.TXT".into(), b"older note".to_vec()),
        ("LIVE.ID".into(), manifest(STALE)),
        ("TRY.IMG".into(), b"leftover trial".to_vec()),
    ]
    .into()
}

fn assert_current(volume: &Volume, image: &[u8], note: &[u8]) {
    let files = volume.files.borrow();
    assert_eq!(files["CURRENT.IMG"], image);
    assert_eq!(files["CURRENT.TXT"], note);
}

fn assert_trial_ready(outcome: Outcome<Vec<u8>>, volume: &Volume, armed: &[u8]) {
    let Outcome::Trial(attempt, image) = outcome else {
        panic!("verified candidate must be ready to launch");
    };
    let files = volume.files.borrow();
    assert_eq!(attempt.token, TOKEN);
    assert_eq!(image, CANDIDATE);
    assert_eq!(files["TRY.IMG"], CANDIDATE);
    assert_eq!(files["LIVE.ID"], armed);
}

#[test]
fn manager_tries_once_and_requires_matching_acknowledgement() {
    let armed = manifest(TOKEN);
    let stage = Some(armed.as_slice());
    let clean = Volume::new(initial(), None);
    // Only the image, then a torn manifest, then an intact manifest with a bad image.
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
    let reboot = Volume::new(clean.snapshot(), None);
    let outcome = run(&reboot, stage, CANDIDATE, None, false).unwrap();
    assert_current(&reboot, CURRENT, BASE);
    assert!(matches!(outcome, Outcome::Current));

    for step in 0..clean.steps.get() {
        for after in [false, true] {
            let volume = Volume::new(initial(), Some((step, after)));
            assert!(matches!(
                run(&volume, stage, CANDIDATE, None, true),
                Err(DEVICE_ERROR)
            ));
            let reboot = Volume::new(volume.snapshot(), None);
            let attempted = reboot.files.borrow().get("LIVE.ID") == Some(&armed);
            assert!(matches!(
                run(&reboot, stage, CANDIDATE, None, false),
                Ok(Outcome::Current)
            ));
            assert_current(&reboot, CURRENT, BASE);
            let outcome = run(&reboot, stage, CANDIDATE, None, true).unwrap();
            if attempted {
                assert_eq!(reboot.files.borrow()["TRY.IMG"], CANDIDATE);
                assert!(matches!(outcome, Outcome::Rollback(TOKEN)));
            } else {
                assert_trial_ready(outcome, &reboot, &armed);
            }

            // Reset again after the handoff, including a stale receipt from a prior trial.
            for ack in [None, Some(STALE_ACK)] {
                let reboot = Volume::new(reboot.snapshot(), None);
                assert!(matches!(
                    run(&reboot, stage, CANDIDATE, ack, false),
                    Ok(Outcome::Current)
                ));
                assert!(matches!(
                    run(&reboot, stage, CANDIDATE, ack, true),
                    Ok(Outcome::Rollback(TOKEN))
                ));
                assert_current(&reboot, CURRENT, BASE);
                assert_eq!(reboot.steps.get(), 0);
            }
        }
    }
}

#[test]
fn manager_resumes_acknowledged_promotion_after_power_loss() {
    let armed = manifest(TOKEN);
    let stage = Some(armed.as_slice());
    let trial = Volume::new(initial(), None);
    assert_trial_ready(
        run(&trial, stage, CANDIDATE, None, true).unwrap(),
        &trial,
        &armed,
    );

    let clean = Volume::new(trial.snapshot(), None);
    let Outcome::Promoted(Manifest { token: TOKEN, .. }) =
        run(&clean, stage, CANDIDATE, Some(ACK), false).unwrap()
    else {
        panic!("matching acknowledgement must promote the retained trial");
    };
    let note = format!(
        "commit {}\ninstalled-by {}\n",
        "a".repeat(40),
        "1".repeat(32)
    );
    assert_current(&clean, CANDIDATE, note.as_bytes());

    for step in 0..clean.steps.get() {
        for after in [false, true] {
            let volume = Volume::new(trial.snapshot(), Some((step, after)));
            assert!(matches!(
                run(&volume, stage, CANDIDATE, Some(ACK), false),
                Err(DEVICE_ERROR)
            ));
            let reboot = Volume::new(volume.snapshot(), None);
            // Installed startup recovers the slot journal before reconciling the live trial.
            slot_transaction::recover(&reboot).unwrap();
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
            let files = reboot.files.borrow();
            assert_eq!(files["PREVIOUS.IMG"], CURRENT);
            assert_eq!(files["PREVIOUS.TXT"], BASE);
            for name in ["NEXT.IMG", "NEXT.TXT", "SLOT.TXN"] {
                assert!(
                    !files.contains_key(name),
                    "{name}, step {step}, after={after}"
                );
            }
        }
    }
}


// These adapters run the old manager's operations until the record implementation
// takes over. The test observations are the bytes offered to the loader.
fn startup(volume: &Volume) -> Result<([u8; 32], Option<[u8; 32]>), Status> {
    slot_transaction::recover(volume)?;
    let files = volume.files.borrow();
    let verify = |name| files.get(name).filter(|bytes| {
        // SAFETY: the fixture exposes the whole image to the production shape check.
        unsafe { crate::bootdisk::looks_like_disk(bytes.as_ptr(), bytes.len()) }
    }).map(|bytes| sha256(bytes));
    Ok((verify("CURRENT.IMG").ok_or(NOT_FOUND)?, verify("PREVIOUS.IMG")))
}

#[derive(Clone, Copy, Debug)]
enum Transition {
    Import,
    Try,
    Promote,
    Commit,
    Rollback,
}

fn apply(volume: &Volume, transition: Transition, armed: &[u8]) -> Result<(), Status> {
    match transition {
        Transition::Import => startup(volume).map(|_| ()),
        Transition::Try => run(volume, Some(armed), CANDIDATE, None, true).map(|_| ()),
        Transition::Promote => run(volume, Some(armed), CANDIDATE, Some(ACK), false).map(|_| ()),
        Transition::Commit => slot_transaction::begin(volume, false),
        Transition::Rollback => slot_transaction::begin(volume, true),
    }
}

fn with_next(mut files: Files) -> Files {
    files.insert("NEXT.IMG".into(), CANDIDATE.to_vec());
    files.insert("NEXT.TXT".into(), b"candidate note".to_vec());
    files
}

#[test]
fn t_c1_each_esp_write_preserves_a_pair() {
    let armed = manifest(TOKEN);
    let imported = Volume::new(initial(), None);
    apply(&imported, Transition::Import, &armed).unwrap();
    let tried = Volume::new(imported.snapshot(), None);
    apply(&tried, Transition::Try, &armed).unwrap();
    let old = (sha256(CURRENT), Some(sha256(PREVIOUS)));
    let promoted = (sha256(CANDIDATE), Some(sha256(CURRENT)));
    let swapped = (sha256(PREVIOUS), Some(sha256(CURRENT)));
    let mut failures = Vec::new();
    for (transition, source, expected) in [
        (Transition::Import, initial(), old),
        (Transition::Try, imported.snapshot(), old),
        (Transition::Promote, tried.snapshot(), promoted),
        (Transition::Commit, with_next(imported.snapshot()), promoted),
        (Transition::Rollback, imported.snapshot(), swapped),
    ] {
        for both_first in [true, false] {
            let mut complete = Volume::new(source.clone(), None);
            complete.rename_order = Some(both_first);
            apply(&complete, transition, &armed).unwrap();
            assert_eq!(startup(&complete), Ok(expected), "{transition:?}");
            for step in 0..complete.steps.get() {
                for after in [false, true] {
                    let mut cut = Volume::new(source.clone(), Some((step, after)));
                    cut.rename_order = Some(both_first);
                    let _ = apply(&cut, transition, &armed);
                    let reboot = Volume::new(cut.snapshot(), None);
                    match startup(&reboot) {
                        Ok(pair) if pair == old || pair == expected => {}
                        result => failures.push(format!("{transition:?}, write {step}, after={after}, both first={both_first}: {result:?}")),
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn t_c2_failed_import_boots_without_writing() {
    for both in [true, false] {
        let mut files = with_next(initial());
        files.insert("SLOT.TXN".into(), b"commit\n".to_vec());
        files.insert("SLOT0.OK".into(), b"done\n".to_vec());
        if both {
            files.insert("PREVIOUS.IMG".into(), CURRENT.to_vec());
        } else {
            files.remove("CURRENT.IMG");
            files.remove("PREVIOUS.IMG");
        }
        let volume = Volume::new(files.clone(), None);
        let expected = if both { CURRENT } else { CANDIDATE };
        assert_eq!(startup(&volume).map(|pair| pair.0), Ok(sha256(expected)), "both={both}");
        assert_eq!(volume.snapshot(), files);
        assert_eq!(volume.steps.get(), 0);
    }
}

const ROOT: &str = "gen-20261127T123456Z-metal";
const INTENT: &[u8] = b"BREENIX-BOOT 3\ntarget x86-metal\nuserland 20261127T123456Z-metal\n";

fn host_manifest(digest: [u8; 32], size: usize) -> Vec<u8> {
    let hex = |bytes: [u8; 32]| bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    std::fs::create_dir_all("target").unwrap();
    let output = std::process::Command::new("python3")
        .args(["-B", "-c", r"
import sys
sys.path.insert(0, '../')
from metal_live_flash import encode_manifest
with open(sys.argv[1], 'wb') as manifest:
    manifest.write(encode_manifest('1' * 32, 'a' * 40, int(sys.argv[2]), *sys.argv[3:]))
"])
        .arg(format!("target/t-c3-{}-{size}.manifest", std::process::id()))
        .args([&size.to_string(), &hex(digest), &hex(sha256(BASE)), ROOT])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let path = format!("target/t-c3-{}-{size}.manifest", std::process::id());
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    bytes
}

fn attempt_text(volume: &Volume) -> Option<Vec<u8>> {
    volume.files.borrow().get("LIVE.ID").cloned()
}

#[test]
fn t_c3_host_version_two_manifest_round_trips_and_checks_the_pair() {
    for (size, length) in [(10_000_000, 256), (536_870_912, 257)] {
        let mut data = vec![0; size];
        data[..CANDIDATE.len()].copy_from_slice(CANDIDATE);
        let armed = host_manifest(sha256(&data), size);
        assert_eq!(armed.len(), length);
        assert_eq!(Manifest::parse(&armed).map(|m| m.size), Some(size));
        let mut volume = Volume::new(initial(), None);
        volume.root_flags = Some(0);
        volume.intent = INTENT;
        assert!(matches!(run(&volume, Some(&armed), &data, None, true), Ok(Outcome::Trial(..))));
        assert_eq!(attempt_text(&volume).as_deref(), Some(armed.as_slice()));
        assert!(matches!(run(&volume, Some(&armed), &[], Some(ACK), false), Ok(Outcome::Promoted(..))));
    }
    let armed = host_manifest(sha256(CANDIDATE), CANDIDATE.len());
    for (flags, intent) in [
        (None, INTENT),
        (Some(1), INTENT),
        (Some(0), b"userland 20261127T123457Z-metal\n".as_slice()),
    ] {
        let mut volume = Volume::new(initial(), None);
        volume.root_flags = flags;
        volume.intent = intent;
        assert!(run(&volume, Some(&armed), CANDIDATE, None, true).is_err());
        assert_eq!(volume.steps.get(), 0);
    }
    let mut volume = Volume::new(initial(), None);
    volume.root_flags = Some(0);
    volume.intent = INTENT;
    let mut too_long = armed;
    too_long.resize(513, b'x');
    assert!(run(&volume, Some(&too_long), CANDIDATE, None, true).is_err());
    assert_eq!(volume.steps.get(), 0);
    assert!(matches!(run(&volume, Some(&manifest(TOKEN)), CANDIDATE, None, true), Ok(Outcome::Trial(..))));
}
