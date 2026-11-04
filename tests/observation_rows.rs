//! SCANOUT_READBACK rows from breengel.md section 20.2 and gpu.md section 1.8.

use libbreenix::window::{validate_scanout_observation, ObservationKind};

fn word(record: &mut [u8; 1024], offset: usize, value: u32) {
    record[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn quad(record: &mut [u8; 1024], offset: usize, value: u64) {
    record[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn record(
    backend: u32,
    boundary: u32,
    status: u32,
    render: u32,
    readback: u32,
    copy: u32,
) -> [u8; 1024] {
    let mut record = [0; 1024];
    for (offset, value) in [
        (0, 2),
        (4, 1024),
        (16, backend),
        (20, boundary),
        (24, 3),
        (112, status),
        (116, render),
        (132, readback),
        (152, copy),
        (156, if backend == 4 { 0 } else { status }),
        (40, 2560),
        (44, 1600),
        (56, 10240),
        (164, 0),
        (128, status),
        (192, status),
    ] {
        word(&mut record, offset, value);
    }
    for (offset, value) in [(80, 41), (104, 61), (120, 73), (184, 97)] {
        quad(&mut record, offset, value);
    }
    if backend == 5 {
        quad(&mut record, 136, 101);
        quad(&mut record, 144, 103);
    }
    record
}

#[test]
fn accepts_the_three_contract_rows() {
    for record in [
        record(4, 4, 0x1100, 0x207, 0x206, 0),
        record(5, 4, 1, 1152, 1104, 1042),
        record(6, 5, 0x60000000, 0xa1971b0c, 0xa0b50300, 0xa0b50300),
    ] {
        assert!(validate_scanout_observation(
            &record,
            2560,
            1600,
            ObservationKind::Composed
        ));
    }
}

#[test]
fn refuses_native_records_with_virtual_boundary_or_status() {
    assert!(!validate_scanout_observation(
        &record(6, 4, 0x60000000, 0xa1971b0c, 0xa0b50300, 0xa0b50300),
        2560,
        1600,
        ObservationKind::Composed
    ));
    for offset in [128, 192] {
        let mut record = record(6, 5, 0x60000000, 0xa1971b0c, 0xa0b50300, 0xa0b50300);
        word(&mut record, offset, 0x1100);
        assert!(
            !validate_scanout_observation(&record, 2560, 1600, ObservationKind::Composed),
            "offset {offset}"
        );
    }
}

#[test]
fn refuses_retired_and_unknown_backends_with_status_one() {
    for backend in [3, 7] {
        assert!(!validate_scanout_observation(
            &record(backend, 4, 1, 1152, 1104, 1042),
            2560,
            1600,
            ObservationKind::Composed
        ));
    }
}

#[test]
fn retains_dimensions_binding_completion_and_poison_checks() {
    for good in composed_rows() {
        let backend = u32::from_le_bytes(good[16..20].try_into().unwrap());
        for (offset, value) in [
            (0, 1),
            (4, 512),
            (20, 0),
            (40, 1280),
            (44, 800),
            (56, 5120),
            (164, 1),
            (128, 0),
            (192, 0),
        ] {
            let mut bad = good;
            word(&mut bad, offset, value);
            assert!(
                !validate_scanout_observation(&bad, 2560, 1600, ObservationKind::Composed),
                "backend {backend}, offset {offset}"
            );
        }
        for offset in [80, 104, 120, 184] {
            let mut bad = good;
            quad(&mut bad, offset, 0);
            assert!(
                !validate_scanout_observation(&bad, 2560, 1600, ObservationKind::Composed),
                "backend {backend}, zero identity/fence at {offset}"
            );
        }
        assert!(!validate_scanout_observation(
            &good,
            1280,
            1600,
            ObservationKind::Composed
        ));
        assert!(!validate_scanout_observation(
            &good,
            2560,
            800,
            ObservationKind::Composed
        ));
    }
}

fn composed_rows() -> [[u8; 1024]; 3] {
    [
        record(4, 4, 0x1100, 0x207, 0x206, 0),
        record(5, 4, 1, 1152, 1104, 1042),
        record(6, 5, 0x60000000, 0xa1971b0c, 0xa0b50300, 0xa0b50300),
    ]
}

#[test]
fn composed_rows_require_draw_copy_and_composite_provenance() {
    for good in composed_rows() {
        let backend = u32::from_le_bytes(good[16..20].try_into().unwrap());
        for (offset, value) in [
            (24, 0),
            (24, 1),
            (24, 2),
            (24, 7),
            (112, 0x60010000),
            (116, 0xa0b50300),
            (132, 0),
            (152, if backend == 4 { 1042 } else { 0 }),
            (156, if backend == 4 { 1 } else { 0 }),
        ] {
            let mut bad = good;
            word(&mut bad, offset, value);
            assert!(
                !validate_scanout_observation(&bad, 2560, 1600, ObservationKind::Composed),
                "backend {backend}, offset {offset}"
            );
        }
        for offset in [136, 144] {
            let mut bad = good;
            quad(&mut bad, offset, if backend == 5 { 0 } else { 101 });
            assert!(!validate_scanout_observation(
                &bad,
                2560,
                1600,
                ObservationKind::Composed
            ));
        }
        if backend == 5 {
            let mut bad = good;
            quad(&mut bad, 136, 41); // dedicated surface must differ from resource
            assert!(!validate_scanout_observation(
                &bad,
                2560,
                1600,
                ObservationKind::Composed
            ));
        }
        let mut plain = good;
        word(&mut plain, 24, 1);
        // Plain scanout does not attest a render or a copy.
        for offset in [112, 116, 152, 156] {
            word(&mut plain, offset, 0);
        }
        quad(&mut plain, 104, 0);
        assert!(validate_scanout_observation(
            &plain,
            2560,
            1600,
            ObservationKind::Plain
        ));
        assert!(!validate_scanout_observation(
            &good,
            2560,
            1600,
            ObservationKind::Plain
        ));
    }
}

// Exact 1,024-byte record slices, after the SUITE_SCANOUT header, from the
// retained 20261104T175252Z-metal uploads; neither retained file is edited.
const STARTUP: &[u8; 1024] = include_bytes!("fixtures/observations/175252Z/startup.bin");
const FINAL: &[u8; 1024] = include_bytes!("fixtures/observations/175252Z/suite-final-panel.bin");

#[test]
fn retained_panels_are_plain_and_startup_has_no_present_fence() {
    assert!(!validate_scanout_observation(
        STARTUP,
        2560,
        1600,
        ObservationKind::Plain
    ));
    assert!(!validate_scanout_observation(
        STARTUP,
        2560,
        1600,
        ObservationKind::Composed
    ));
    assert!(validate_scanout_observation(
        FINAL,
        2560,
        1600,
        ObservationKind::Plain
    ));
    assert!(!validate_scanout_observation(
        FINAL,
        2560,
        1600,
        ObservationKind::Composed
    ));
    let mut composed = *FINAL;
    word(&mut composed, 24, 3);
    word(&mut composed, 152, 0);
    assert!(!validate_scanout_observation(
        &composed,
        2560,
        1600,
        ObservationKind::Composed
    ));
}
