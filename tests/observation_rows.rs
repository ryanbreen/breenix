//! SCANOUT_READBACK rows from breengel.md section 20.2 and gpu.md section 1.8.

use libbreenix::window::validate_scanout_observation;

fn word(record: &mut [u8; 1024], offset: usize, value: u32) {
    record[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn quad(record: &mut [u8; 1024], offset: usize, value: u64) {
    record[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn record(backend: u32, boundary: u32, status: u32) -> [u8; 1024] {
    let mut record = [0; 1024];
    for (offset, value) in [
        (0, 2),
        (4, 1024),
        (16, backend),
        (20, boundary),
        (40, 2560),
        (44, 1600),
        (56, 10240),
        (164, 0),
        (128, status),
        (192, status),
    ] {
        word(&mut record, offset, value);
    }
    for (offset, value) in [(80, 41), (120, 73), (184, 97)] {
        quad(&mut record, offset, value);
    }
    record
}

#[test]
fn accepts_the_three_contract_rows() {
    for record in [
        record(4, 4, 0x1100),
        record(5, 4, 1),
        record(6, 5, 0x60000000),
    ] {
        assert!(validate_scanout_observation(&record, 2560, 1600));
    }
}

#[test]
fn refuses_native_records_with_virtual_boundary_or_status() {
    assert!(!validate_scanout_observation(
        &record(6, 4, 0x60000000),
        2560,
        1600
    ));
    for offset in [128, 192] {
        let mut record = record(6, 5, 0x60000000);
        word(&mut record, offset, 0x1100);
        assert!(
            !validate_scanout_observation(&record, 2560, 1600),
            "offset {offset}"
        );
    }
}

#[test]
fn refuses_retired_and_unknown_backends_with_status_one() {
    for backend in [3, 7] {
        assert!(!validate_scanout_observation(
            &record(backend, 4, 1),
            2560,
            1600
        ));
    }
}

#[test]
fn retains_dimensions_binding_completion_and_poison_checks() {
    for (backend, boundary, status) in [(4, 4, 0x1100), (5, 4, 1), (6, 5, 0x60000000)] {
        let good = record(backend, boundary, status);
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
                !validate_scanout_observation(&bad, 2560, 1600),
                "backend {backend}, offset {offset}"
            );
        }
        for offset in [80, 120, 184] {
            let mut bad = good;
            quad(&mut bad, offset, 0);
            assert!(
                !validate_scanout_observation(&bad, 2560, 1600),
                "backend {backend}, zero identity/fence at {offset}"
            );
        }
        assert!(!validate_scanout_observation(&good, 1280, 1600));
        assert!(!validate_scanout_observation(&good, 2560, 800));
    }
}
