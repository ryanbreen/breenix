use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use crate::live_flash::{Manifest, Token};
use crate::live_sequence::{process, Media, Outcome};
use crate::sha256::sha256;
use crate::slot_transaction::{self, Store};
use crate::uefi::{Status, DEVICE_ERROR, NOT_FOUND, VOLUME_CORRUPTED};

const CURRENT: &[u8] = b"known-good image";
const BASE: &[u8] = b"known-good note";
const CANDIDATE: &[u8] = b"candidate image";
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
}

impl Volume {
    fn new(files: Files, cut: Option<(usize, bool)>) -> Self {
        Self {
            files: RefCell::new(files),
            pending_image: RefCell::new(None),
            pending_marker: RefCell::new(None),
            cut,
            steps: Cell::new(0),
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
        ("PREVIOUS.IMG".into(), b"older image".to_vec()),
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
        (stage, b"damaged!! image".as_slice()),
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
