//! Script ordering and failure cleanup over the production pointer sequencer.
extern crate alloc;
extern crate self as libbui;
#[path = "../libs/libuitest/src/input.rs"]
pub mod input;
#[path = "../libs/libbui/src/tree.rs"]
pub mod tree;
use input::{Backend, Delivery, Pointer, Resolved, Target};

#[derive(Default)]
struct Fake {
    log: Vec<String>,
    held: bool,
    fail_motion: bool,
    fail_hit: bool,
    version: u64,
}
impl Backend for Fake {
    fn resolve(&mut self, target: &Target) -> Result<Resolved, String> {
        self.version += 1;
        self.log
            .push(format!("resolve:{}:{}", target.key, self.version));
        let node = tree::Node {
            key: target.key.clone(),
            parent: None,
            role: tree::Role::Button,
            name: String::new(),
            value: serde_json::Value::Null,
            bounds: [10, 20, 100, 40],
            clip: [20, 30, 80, 20],
            flags: vec![tree::Flag::Visible],
        };
        Ok(Resolved {
            window: target.window,
            point: input::click_target(&node, (100, 200), &[[100, 200, 640, 480]])?,
            scene: self.version,
            commit: self.version,
        })
    }
    fn move_to(&mut self, point: (i32, i32)) -> Result<(), String> {
        assert_eq!(
            input::home_then_move((1000, 800), point)?,
            [(-1000, -800), point]
        );
        self.log.push(format!("position:{point:?}"));
        Ok(())
    }
    fn confirm(&mut self, resolved: &Resolved) -> Result<(), String> {
        self.log.push(format!("hit:{}", resolved.scene));
        if self.fail_hit {
            Err("hit-test changed window".into())
        } else {
            Ok(())
        }
    }
    fn button(&mut self, pressed: bool, window: u64) -> Result<Delivery, String> {
        self.held = pressed;
        self.log.push(format!("button:{pressed}"));
        Ok(Delivery {
            serial: self.log.len() as u64,
            window,
        })
    }
    fn motion(&mut self, delta: (i32, i32), window: u64) -> Result<Delivery, String> {
        assert!(self.held);
        self.log.push(format!("motion:{delta:?}"));
        if self.fail_motion {
            Err("forced motion failure".into())
        } else {
            Ok(Delivery {
                serial: self.log.len() as u64,
                window,
            })
        }
    }
    fn wheel(&mut self, dy: i32, window: u64) -> Result<Delivery, String> {
        self.log.push(format!("wheel:{dy}"));
        Ok(Delivery {
            serial: self.log.len() as u64,
            window,
        })
    }
}
#[test]
fn click_and_scroll_resolve_again_and_confirm_before_injection() {
    let mut p = Pointer::new(Fake::default());
    let target = Target::new(7, "accept");
    let click = p.click(&target).unwrap();
    assert_eq!(click.point, (160, 240));
    assert_eq!(click.press.window, 7);
    assert_eq!(click.release.window, 7);
    assert_eq!(
        p.backend_mut().log,
        [
            "resolve:accept:1",
            "position:(160, 240)",
            "hit:1",
            "button:true",
            "button:false"
        ]
    );
    assert_eq!(p.scroll(&target, -3).unwrap().window, 7);
    assert_eq!(
        &p.backend_mut().log[5..],
        [
            "resolve:accept:2",
            "position:(160, 240)",
            "hit:2",
            "wheel:-3"
        ]
    );
    p.backend_mut().fail_hit = true;
    assert!(p.click(&target).is_err());
    assert!(!p.backend_mut().held);
    assert_eq!(p.backend_mut().log.last().unwrap(), "hit:3");
}
#[test]
fn drag_keeps_capture_across_changes_and_releases_on_failure() {
    let mut p = Pointer::new(Fake::default());
    let target = Target::new(7, "amount");
    let result = p
        .drag(&target, &[(3, 0); 10], |index, backend| {
            if index == 0 || index == 5 {
                backend.version += 1;
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(result.motions.len(), 10);
    assert!(!p.backend_mut().held);
    assert!(result.motions.windows(2).all(|w| w[0].serial < w[1].serial));
    assert_eq!(
        p.backend_mut()
            .log
            .iter()
            .filter(|s| s.starts_with("resolve:"))
            .count(),
        1
    );
    p.backend_mut().fail_motion = true;
    assert!(p
        .drag(&target, &[(3, 0)], |_, _| Ok(()))
        .unwrap_err()
        .contains("forced motion"));
    assert!(!p.backend_mut().held);
    assert_eq!(p.backend_mut().log.last().unwrap(), "button:false");
    assert!(p
        .drag(&target, &[(3, 0)], |_, _| Err("hook failure".into()))
        .is_err());
    assert!(!p.backend_mut().held);
}

#[test]
fn exposure_subtracts_higher_frames_and_screen_edges() {
    assert_eq!(
        input::exposed_region([32, 68, 640, 400], [0, 0, 1280, 800], &[[80, 88, 644, 436]]),
        vec![[32, 68, 640, 20], [32, 88, 48, 380]]
    );
    assert!(input::exposed_region([0, 0, 10, 10], [0, 0, 100, 100], &[[0, 0, 20, 20]]).is_empty());
    assert_eq!(
        input::exposed_region([-10, -5, 20, 20], [0, 0, 100, 100], &[]),
        vec![[0, 0, 10, 15]]
    );
}
