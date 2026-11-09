//! Exercise the scenario oracle with the real desktop producer and widget raster.
#[path = "../userspace/programs/src/uitest_suite/proof_wm_checks.rs"]
mod checks;
#[path = "../userspace/programs/src/bwm/desktop_tree.rs"]
mod desktop_tree;
#[path = "../userspace/programs/src/bwm/stack.rs"]
mod stack;
use crate::{surface::Surface, widgets::Widgets, KeyInput};
use libpixcheck::{Format, Frame};

fn desktop(scale: u32, top: usize) -> libbui::tree::Tree {
    let s = i64::from(scale);
    let mut stack = stack::Stack::new();
    stack.add(11, false);
    stack.add(22, false);
    stack.raise([11, 22][top]);
    desktop_tree::build(
        &stack,
        [
            desktop_tree::WindowState {
                id: 11,
                title: "wm scenario lower",
                owner: 101,
                bounds: [30 * s, 38 * s, 644 * s, 436 * s],
                chromeless: false,
            },
            desktop_tree::WindowState {
                id: 22,
                title: "wm scenario upper",
                owner: 202,
                bounds: [80 * s, 88 * s, 644 * s, 436 * s],
                chromeless: false,
            },
        ],
        [1280 * s, 800 * s],
        7,
    )
}
fn frame(bytes: &[u8], scale: u32) -> Frame<'_> {
    let s = scale as usize;
    Frame::new(bytes, 1280 * s, 800 * s, 1280 * s * 4, Format::Bgrx).unwrap()
}
fn composed(scale: u32, top: usize) -> Vec<u8> {
    let s = scale as usize;
    let mut pixels = vec![0; 1280 * s * 800 * s * 4];
    for index in [1 - top, top] {
        let mut surface = Surface::new(scale);
        Widgets::new().draw(&mut surface);
        let (ox, oy) = [(32 * s, 72 * s), (82 * s, 122 * s)][index];
        for y in 0..400 * s {
            for x in 0..640 * s {
                let c = surface.get_pixel(x, y);
                let at = ((oy + y) * 1280 * s + ox + x) * 4;
                pixels[at..at + 4].copy_from_slice(&[c.b, c.g, c.r, 255]);
            }
        }
    }
    pixels
}
#[test]
fn desktop_oracle_rejects_missing_raise_focus_and_literal_geometry_lies() {
    for scale in [1, 2] {
        for top in [0, 1] {
            let good = desktop(scale, top);
            let verify = |tree: &libbui::tree::Tree| {
                checks::desktop(
                    tree,
                    [11, 22],
                    [101, 202],
                    scale,
                    [1280 * scale, 800 * scale],
                    top,
                )
            };
            verify(&good).unwrap();
            assert!(verify(&desktop(scale, 1 - top)).is_err());
            for field in ["name", "bounds", "z", "focus"] {
                let mut bad = good.clone();
                let node = &mut bad.children[1];
                match field {
                    "name" => node.name.push('x'),
                    "bounds" => node.bounds[0] += 1,
                    "z" => node.value["z"] = serde_json::json!(0),
                    "focus" => node.flags.retain(|f| *f != libbui::tree::Flag::Focused),
                    _ => unreachable!(),
                }
                assert!(verify(&bad).is_err(), "accepted {field} lie");
            }
        }
    }
}
#[test]
fn text_oracle_refuses_misrouted_or_dropped_input() {
    for scale in [1, 2] {
        let mut lower = Widgets::new();
        let mut upper = Widgets::new();
        lower.key(&KeyInput::text("alpha"));
        checks::entry(&lower.committed_tree(scale), "alpha", scale).unwrap();
        checks::entry(&upper.committed_tree(scale), "", scale).unwrap();
        assert!(checks::entry(&upper.committed_tree(scale), "beta", scale).is_err());
        upper.key(&KeyInput::text("beta"));
        checks::entry(&upper.committed_tree(scale), "beta", scale).unwrap();
        assert!(checks::entry(&lower.committed_tree(scale), "alphabeta", scale).is_err());
    }
}
#[test]
fn structural_screen_oracle_refuses_stale_overlap_and_damaged_exposed_windows() {
    for scale in [1, 2] {
        for top in [0, 1] {
            let good = composed(scale, top);
            checks::screen(frame(&good, scale), scale, top).unwrap();
            assert!(checks::screen(frame(&composed(scale, 1 - top), scale), scale, top).is_err());
            for (x, y) in [(40, 152), (700, 480), (364, 166)] {
                let mut bad = good.clone();
                let at = ((y * scale * 1280 * scale + x * scale) * 4) as usize;
                bad[at..at + 3].fill(255);
                assert!(checks::screen(frame(&bad, scale), scale, top).is_err());
            }
        }
    }
}

#[test]
fn desktop_oracle_rejects_minimized_and_removed_windows_and_accepts_restored_policy() {
    let mut policy = stack::Stack::new();
    policy.adopt(&[(11, 0, false, false, false), (22, 1, false, true, false)]);
    let project = |policy: &stack::Stack| {
        desktop_tree::build(
            policy,
            [
                desktop_tree::WindowState {
                    id: 11,
                    title: "wm scenario lower",
                    owner: 101,
                    bounds: [30, 38, 644, 436],
                    chromeless: false,
                },
                desktop_tree::WindowState {
                    id: 22,
                    title: "wm scenario upper",
                    owner: 202,
                    bounds: [80, 88, 644, 436],
                    chromeless: false,
                },
            ],
            [1280, 800],
            9,
        )
    };
    let verify = |policy: &stack::Stack, top| {
        checks::desktop(&project(policy), [11, 22], [101, 202], 1, [1280, 800], top)
    };
    verify(&policy, 1).unwrap();
    let next = policy.cycle(false).unwrap();
    assert_eq!(next, 11);
    let transition = policy.raise(next);
    let mut notices = Vec::new();
    stack::publish_focus(transition, |notice| notices.push(notice), || ());
    assert_eq!(
        notices,
        [stack::FocusNotice::Lost(22), stack::FocusNotice::Gained(11)]
    );
    assert_eq!(policy.state(11), Some((1, false, true)));
    verify(&policy, 0).unwrap();
    policy.minimize(11);
    assert!(verify(&policy, 1).is_err());
    policy.restore(11);
    verify(&policy, 0).unwrap();
    policy.remove(22);
    assert!(verify(&policy, 0).is_err());
}

#[test]
fn composed_front_accepts_no_change_metadata_and_refuses_stale_pixels() {
    // Pixel front 19 remains generation 7 after metadata-only release ACK 20.
    assert!(checks::front_matches(19, 7, 7, 20, 7));
    assert!(checks::front_matches(19, 7, 7, 19, 7));
    assert!(!checks::front_matches(19, 7, 7, 18, 7), "pre-front projection");
    assert!(!checks::front_matches(19, 7, 7, 20, 6), "stale pixels");
    assert!(!checks::front_matches(19, 7, 7, 20, 8), "uncompared newer pixels");
    assert!(!checks::front_matches(19, 7, 8, 20, 8), "front moved during capture");
}
