#[path = "../userspace/programs/src/bwm/cursor.rs"] pub mod cursor;
#[path = "../userspace/programs/src/bwm/settings.rs"] pub mod settings;
#[path = "../userspace/programs/src/uitest_canonical.rs"] pub mod scene;
#[path = "../libs/libuitest/src/fixtures.rs"] pub mod fixtures;
#[path = "../userspace/programs/src/uitest_app_support.rs"] pub mod app;
use std::path::Path;

#[test]
fn cursor_option_is_session_only_and_absence_keeps_main_defaults() {
    let root = std::env::temp_dir().join(format!("cursor-policy-{}", std::process::id()));
    std::fs::create_dir_all(root.join("etc")).unwrap();
    assert!(cursor::load(None).unwrap());
    assert!(cursor::load(Some(&root)).unwrap());
    let baseline = settings::Settings::parse("font.size=14\nclock.visible=true\n").unwrap();
    assert_eq!(baseline, settings::Settings::default());
    fixtures::prepare(&root).unwrap();
    assert!(!cursor::load(Some(&root)).unwrap());
    assert_eq!(settings::Settings::load(&root.join("etc/bwm.conf")).unwrap(), baseline);
    assert_eq!(cursor::choose(false, (12, 34, 0)), None);
    assert_eq!(cursor::choose(true, (12, 34, 0)), Some((12, 34, 0)));
    std::fs::write(root.join("etc/uitest-cursor.conf"), "visible=true\n").unwrap();
    assert!(cursor::load(Some(&root)).unwrap());
    // A session file cannot affect an ordinary desktop's selection.
    assert!(cursor::load(None).unwrap());
    std::fs::write(root.join("etc/uitest-cursor.conf"), "visible=maybe\n").unwrap();
    assert!(cursor::load(Some(&root)).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

fn rendered() -> Vec<u8> {
    use libgfx_ref::{color::Color, framebuf::{Format, FrameBuf}, shapes};
    let mut frame = FrameBuf::new_filled(scene::WIDTH, scene::HEIGHT, Format::BGRX, Color::rgb(0,0,0)).unwrap();
    scene::draw(|x,y,w,h,[r,g,b]| shapes::fill_rect(&mut frame,x,y,w,h,Color::rgb(r,g,b)));
    let mut bytes = Vec::new();
    for y in 0..scene::HEIGHT { for x in 0..scene::WIDTH {
        let c = frame.get_pixel(x,y); bytes.extend_from_slice(&[c.b,c.g,c.r,255]);
    }}
    bytes
}

#[test]
fn canonical_scene_matches_committed_host_pin_and_detects_cursor_pixels() {
    let pixels = rendered();
    let digest = fixtures::digest(&pixels);
    let mut planted = pixels.clone();
    planted[(100*scene::WIDTH+160)*4..][..4].copy_from_slice(&[255;4]);
    let provenance: serde_json::Value = serde_json::from_str(include_str!("../docs/suites/frames/PROVENANCE.json")).unwrap();
    let rows: Vec<_> = provenance["references"].as_array().unwrap().iter()
        .filter(|r| r["case"] == "frames/cross-target").collect();
    assert_eq!(rows.len(), 2);
    let root = Path::new(file!()).parent().unwrap().join("../docs/suites/frames");
    for (arch, target, pin_path) in [
        ("x86_64", "x86-metal", "ui-testing/canonical-x86-metal.sha256"),
        ("aarch64", "arm64-qemu", "ui-testing/canonical-arm64-qemu.sha256"),
    ] {
        let matching: Vec<_> = rows.iter().filter(|r| r["architecture"] == arch).collect();
        assert_eq!(matching.len(), 1, "expected one row for {arch}");
        let row = matching[0];
        assert_eq!(row["target"], target);
        assert_eq!(row["pin_path"], pin_path);
        assert_eq!(row["pixel_sha256"], digest);
        assert_eq!(std::fs::read_to_string(root.join(pin_path)).unwrap().trim(), digest);
        assert_ne!(fixtures::digest(&planted), digest);
        assert_eq!(row["generator"], "tools/gfx-refgen --bin ui-canonical");
        assert_eq!(fixtures::digest(&std::fs::read(root.join(row["path"].as_str().unwrap())).unwrap()), row["sha256"]);
        assert!(row.get("arm_gap").is_none());
    }
    assert_eq!(rows[0]["generator_sources"], rows[1]["generator_sources"]);
    assert!(app::parse(&["canonical", "--canonical"]).unwrap().canonical);
    assert!(app::parse(&["canonical", "--canonical", "--canonical"]).is_err());
    assert!(!app::parse(&["markers"]).unwrap().canonical);
}
