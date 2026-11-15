#[path = "../userspace/programs/src/bwm/settings.rs"]
pub mod settings;

use settings::{with_display, Settings};

#[test]
fn display_settings_parse_with_nonfatal_warnings() {
    let defaults = Settings::parse("").unwrap();
    assert_eq!(defaults.display_mode, None);
    assert_eq!(defaults.display_scale, None);
    assert!(defaults.warnings.is_empty());
    let valid = Settings::parse("font.size=12\nclock.visible=false\ndisplay.mode=1920x1080\ndisplay.scale=2\n").unwrap();
    assert_eq!(valid.display_mode, Some((1920, 1080)));
    assert_eq!(valid.display_scale, Some(2));
    assert_eq!(valid.font_size, 12.0);
    assert!(!valid.clock_visible);
    assert!(valid.warnings.is_empty());
    assert_eq!(Settings::parse("display.scale=1\n").unwrap().display_scale, Some(1));
    for data in ["1024x", "0x768", " 1024x768", "1024x768 ", "1024X768", "4294967296x768"] {
        let parsed = Settings::parse(&format!("display.mode={data}\n")).unwrap();
        assert_eq!(parsed.display_mode, None, "{data:?}");
        assert_eq!(parsed.warnings.len(), 1, "{data:?}");
        assert!(parsed.warnings[0].contains("display.mode"));
    }
    for data in ["3", " 1", "2 ", "0"] {
        let parsed = Settings::parse(&format!("display.scale={data}\n")).unwrap();
        assert_eq!(parsed.display_scale, None, "{data:?}");
        assert_eq!(parsed.warnings.len(), 1, "{data:?}");
        assert!(parsed.warnings[0].contains("display.scale"));
    }
    for bad in ["display.mode=bad\ndisplay.mode=1024x768\n", "display.scale=3\ndisplay.scale=1\n", "unknown=1\n", "clock.visible=yes\n", "font.size=NaN\n"] {
        assert!(Settings::parse(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn display_updates_preserve_other_lines_byte_for_byte() {
    let original = "# configuration\r\nfont.size=12\n\nclock.visible=false\r\ndisplay.mode=800x600\ndisplay.scale=1\n# end\n";
    assert_eq!(with_display(original, (1920, 1080), 2), "# configuration\r\nfont.size=12\n\nclock.visible=false\r\ndisplay.mode=1920x1080\ndisplay.scale=2\n# end\n");
    assert_eq!(with_display("# untouched\nfont.size=14\n", (1024, 768), 1), "# untouched\nfont.size=14\ndisplay.mode=1024x768\ndisplay.scale=1\n");
    assert_eq!(with_display("# no newline", (640, 480), 2), "# no newline\ndisplay.mode=640x480\ndisplay.scale=2\n");
}
