//! Literal clock-page checks from BreenGL §9.1, independent of the provider.
#[path = "../src/clock.rs"]
mod clock;

#[test]
fn shared_page_tracks_epochs_without_reading_real_time() {
    use std::io::{Seek, SeekFrom, Write};
    let path = std::env::temp_dir().join(format!("breengel-clock-{}", std::process::id()));
    let mut page = [0u8; 4096];
    page[..8].copy_from_slice(b"BGCLK001");
    page[8..16].copy_from_slice(&7u64.to_le_bytes());
    page[16..24].copy_from_slice(&123_456_789u64.to_le_bytes());
    page[24..32].copy_from_slice(&1_767_225_600_000_000_000u64.to_le_bytes());
    std::fs::write(&path, page).unwrap();
    std::env::set_var("BREENIX_UI_CLOCK", &path);
    assert_eq!(clock::now_ns(), 123_456_789);
    assert_eq!(clock::epoch(), 7);
    assert_eq!(clock::wall_ns(), 1_767_225_600_123_456_789);
    assert_eq!(clock::real_clock_reads(), 0);
    let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.seek(SeekFrom::Start(16)).unwrap();
    file.write_all(&250_000_000u64.to_le_bytes()).unwrap();
    assert_eq!(clock::now_ns(), 250_000_000);
    // A fresh session can replace the page at the very same configured path.
    page[8..16].copy_from_slice(&8u64.to_le_bytes());
    page[16..24].copy_from_slice(&987u64.to_le_bytes());
    let replacement = path.with_extension("next");
    std::fs::write(&replacement, page).unwrap();
    std::fs::rename(replacement, &path).unwrap();
    assert_eq!(clock::now_ns(), 987);
    assert_eq!(clock::epoch(), 8);
    std::fs::remove_file(&path).unwrap();
    assert_eq!(clock::now_ns(), 987, "unlinked mapped clock remains usable");
    assert_eq!(clock::epoch(), 8);
    let before = clock::real_clock_reads();
    assert!(clock::real_now_ns() > 0);
    assert_eq!(clock::real_clock_reads(), before + 1);
    std::env::remove_var("BREENIX_UI_CLOCK");
    let first = clock::now_ns();
    assert!(clock::now_ns() >= first);
    assert_eq!(clock::epoch(), 0);
    assert!(clock::wall_ns() > 1_767_225_600_000_000_000);
    // A cached inode still has to be long enough to hold the clock header.
    std::fs::write(&path, page).unwrap();
    std::env::set_var("BREENIX_UI_CLOCK", &path);
    assert_eq!(clock::now_ns(), 987);
    std::fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(8).unwrap();
    assert!(std::panic::catch_unwind(clock::now_ns).is_err(), "truncated cached page must be rejected");
    std::env::remove_var("BREENIX_UI_CLOCK");
    std::fs::remove_file(&path).unwrap();

}
