//! Literal config and UI-clock reload scenarios from BreenGL §9.
#[path = "../src/clock.rs"]
pub mod clock;
#[path = "../src/font.rs"]
pub mod font;
use font::{FontChange, FontConfig, FontWatchError, FontWatcher};

#[test]
fn reject_partial_duplicate_unknown_and_invalid_sizes() {
    let good = "mono.font=/mono.ttf\nmono.size=16\ndisplay.font=/display.ttf\ndisplay.size=24\n";
    assert!(FontConfig::parse(good).is_ok());
    for bad in [
        good.trim_end().to_string(),
        format!("{good}mono.size=18\n"),
        format!("{good}extra=yes\n"),
        good.replace("=16\n", "=NaN\n"),
        good.replace("=16\n", "=73\n"),
        good.replace("=16\n", "=5\n"),
        good.replace("mono.size=16", " mono.size=16"),
        good.replace("mono.size=16", "mono.size =16"),
        good.replace("mono.size=16", "mono.size= 16"),
        good.replace("mono.size=16", "mono.size=16 "),
        good.replace("mono.size=16", "mono.size=+16"),
        good.replace("mono.size=16", "mono.size=1e1"),
        good.replace("display.size=24\n", ""),
    ] {
        assert_eq!(
            FontConfig::parse(&bad),
            Err(FontWatchError::Config),
            "{bad:?}"
        );
    }
}
#[test]
fn reload_is_paced_independent_atomic_and_tracks_inode_replacements() {
    use std::io::{Seek, SeekFrom, Write};
    let dir = std::env::temp_dir().join(format!("breengel-font-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("etc")).unwrap();
    let mono = dir.join("mono.ttf");
    let display = dir.join("display.ttf");
    let mut original = include_bytes!("../../../fonts/DejaVuSansMono.ttf").to_vec();
    original.resize(include_bytes!("../../../fonts/DejaVuSans.ttf").len(), 0);
    std::fs::write(&mono, original).unwrap();
    std::fs::write(&display, include_bytes!("../../../fonts/DejaVuSans.ttf")).unwrap();
    let conf = dir.join("etc/fonts.conf");
    let config = |size| {
        format!(
            "mono.font={}\nmono.size={size}\ndisplay.font={}\ndisplay.size=24\n",
            mono.display(),
            display.display()
        )
    };
    std::fs::write(&conf, config(16)).unwrap();
    let page = dir.join("clock");
    let mut bytes = [0u8; 4096];
    bytes[..8].copy_from_slice(b"BGCLK001");
    std::fs::write(&page, bytes).unwrap();
    std::env::set_var("BREENIX_UI_CLOCK", &page);
    std::env::set_var("BREENIX_CONFIG_ROOT", &dir);
    let step = |ns: u64| {
        let mut f = std::fs::OpenOptions::new().write(true).open(&page).unwrap();
        f.seek(SeekFrom::Start(16)).unwrap();
        f.write_all(&ns.to_le_bytes()).unwrap();
    };
    let mut w = FontWatcher::try_new().unwrap();
    std::fs::write(&conf, config(18)).unwrap();
    step(249_999_999);
    assert_eq!(w.poll(), Ok(None));
    assert_eq!(w.mono_size(), 16.0);
    step(250_000_000);
    assert_eq!(
        w.poll(),
        Ok(Some(FontChange {
            mono: true,
            display: false
        }))
    );
    assert_eq!(w.mono_size(), 18.0);
    std::fs::write(
        &conf,
        "mono.font=/missing\nmono.size=18\ndisplay.font=/missing\ndisplay.size=24\n",
    )
    .unwrap();
    step(500_000_000);
    assert_eq!(w.poll(), Err(FontWatchError::Font("/missing".into())));
    assert_eq!(w.mono_size(), 18.0);
    std::fs::write(&conf, "mono.font=/cut").unwrap();
    step(750_000_000);
    assert_eq!(w.poll(), Err(FontWatchError::Config));
    assert_eq!(w.mono_size(), 18.0);
    for i in 0..5 {
        std::fs::write(dir.join("replacement"), config(20 + i)).unwrap();
        std::fs::rename(dir.join("replacement"), &conf).unwrap();
        step(1_000_000_000 + i * 250_000_000);
        assert_eq!(
            w.poll(),
            Ok(Some(FontChange {
                mono: true,
                display: false
            }))
        );
        assert_eq!(w.mono_size(), (20 + i) as f32);
    }
    let before = std::fs::metadata(&mono).unwrap();
    std::fs::write(
        dir.join("replacement"),
        include_bytes!("../../../fonts/DejaVuSans.ttf"),
    )
    .unwrap();
    std::fs::File::options().write(true).open(dir.join("replacement")).unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(before.modified().unwrap())).unwrap();
    let after = std::fs::metadata(dir.join("replacement")).unwrap();
    use std::os::unix::fs::MetadataExt;
    assert_eq!((before.len(), before.mtime(), before.mtime_nsec()), (after.len(), after.mtime(), after.mtime_nsec()));
    assert_ne!(before.ino(), after.ino());
    std::fs::rename(dir.join("replacement"), &mono).unwrap();
    step(2_250_000_000);
    assert_eq!(
        w.poll(),
        Ok(Some(FontChange {
            mono: true,
            display: false
        }))
    );
    // A client can be drawing with a taken font while a reload is applied.
    // Returning that old font must not replace the newly published one.
    let held = w.take_mono_font().unwrap();
    std::fs::write(
        dir.join("replacement"),
        include_bytes!("../../../fonts/DejaVuSansMono.ttf"),
    )
    .unwrap();
    std::fs::rename(dir.join("replacement"), &mono).unwrap();
    step(2_500_000_000);
    assert_eq!(
        w.poll(),
        Ok(Some(FontChange {
            mono: true,
            display: false
        }))
    );
    let glyph = w.mono_font().unwrap().glyph_index('W');
    let new_advance = w.mono_font().unwrap().advance_width(glyph, 24.0);
    assert_ne!(held.advance_width(held.glyph_index('W'), 24.0), new_advance);
    w.put_mono_font(Some(held));
    assert_eq!(
        w.mono_font().unwrap().advance_width(glyph, 24.0),
        new_advance,
        "returning an old client font overwrote the reload"
    );
    assert_eq!(clock::real_clock_reads(), 0);
    std::env::remove_var("BREENIX_UI_CLOCK");
    std::env::remove_var("BREENIX_CONFIG_ROOT");
    std::fs::remove_dir_all(dir).unwrap();
}
