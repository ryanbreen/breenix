//! Read bwm's live tree against literal two-app expectations.
use breenix_suite::suite::{check, CaseResult};
use breengel::{tree::{Flag, Role, Tree}, tree_page::Page};
use libbreenix::{memory, window::{self, abi::{self, op}}};
use libuitest::Session;
use std::time::{Duration, Instant};

struct Reader(abi::MapTree);
impl Reader {
    fn attach() -> Result<Self, String> {
        let mut map = abi::MapTree::default();
        window::call(op::MAP_TREE, &mut map).map_err(|e| e.to_string())?;
        Ok(Self(map))
    }
    fn snapshot(&self) -> Result<Option<Tree>, String> {
        // MAP_TREE id 0 supplies an aligned, read-only mapping held by this reader.
        unsafe { Page::from_mapping(self.0.addr as *mut u8, self.0.len as usize)? }.snapshot()
    }
}
impl Drop for Reader {
    fn drop(&mut self) { let _ = memory::munmap(self.0.addr as *mut u8, self.0.len as usize); }
}
fn status() -> Result<abi::CompositorStatus, String> {
    let mut status = abi::CompositorStatus::default();
    window::call(op::COMPOSITOR_STATUS, &mut status).map_err(|e| e.to_string())?;
    Ok(status)
}
fn expect(tree: &Tree, ids: [u64; 2], owners: [u64; 2], scale: i64,
    screen: [i64; 2], minimized: bool, scene: u64) -> Result<(), String> {
    let mismatch = |field: &str| format!("tree-mismatch desktop field={field} tree={tree:?}");
    if tree.root.key != "desktop" || tree.root.role != Role::Canvas
        || tree.root.bounds != [0,0,screen[0],screen[1]]
        || tree.root.value != serde_json::json!({"scene_version":scene})
        || tree.root.flags.contains(&Flag::Truncated) { return Err(mismatch("root/scene_version")); }
    if tree.children.len() != 2 { return Err(mismatch("window-count")); }
    for (index, (title, bounds)) in [
        ("wm tree lower", [30*scale,38*scale,320+4*scale,200+36*scale]),
        ("wm tree upper", [80*scale,88*scale,320+4*scale,200+36*scale]),
    ].into_iter().enumerate() {
        let node = &tree.children[index];
        let hidden = minimized && index == 1;
        let focused = if minimized { index == 0 } else { index == 1 };
        let mut flags = vec![Flag::Enabled];
        if !hidden { flags.push(Flag::Visible); }
        if focused { flags.push(Flag::Focused); }
        if node.key != format!("window-{}", ids[index]) || node.parent.as_deref() != Some("desktop")
            || node.role != Role::Window || node.name != title || node.bounds != bounds
            || node.clip != if hidden { [bounds[0],bounds[1],0,0] } else { bounds }
            || node.flags != flags
            || node.value != serde_json::json!({"id":ids[index],"owner":owners[index],
                "z":index,"minimized":hidden,"chromeless":false}) {
            return Err(mismatch(&format!("window-{index} title/bounds/z/focus/minimized/owner")));
        }
    }
    Ok(())
}
fn wait(reader: &Reader, ids: [u64; 2], owners: [u64; 2], scale: i64,
    screen: [i64; 2], minimized: bool) -> Result<(), String> {
    let end = Instant::now() + Duration::from_secs(2);
    let mut last = "unpublished compositor tree".to_string();
    loop {
        let before = status()?;
        if let Some(tree) = reader.snapshot()? {
            let after = status()?;
            if before.claim_epoch == after.claim_epoch && before.scene_version == after.scene_version {
                match expect(&tree, ids, owners, scale, screen, minimized, after.scene_version) {
                    Ok(()) => return Ok(()),
                    Err(error) => last = error,
                }
            }
        }
        if Instant::now() >= end { return Err(last); }
        libbreenix::time::sleep_ms(1).map_err(|e| e.to_string())?;
    }
}
pub fn wm_tree() -> CaseResult {
    let mut session = Session::desktop()?;
    let result = (|| -> Result<(), String> {
        let lower = session.launch_app(&[b"/usr/libexec/breenix/uitest-app_test", b"wm tree lower"], 0)?;
        let lower_id = session.wait_window("wm tree lower")?.id;
        let upper = session.launch_app(&[b"/usr/libexec/breenix/uitest-app_test", b"wm tree upper"], 0)?;
        let upper_id = session.wait_window("wm tree upper")?.id;
        let mode = libbreenix::gpu::current_mode().map_err(|e| e.to_string())?;
        let scale = i64::from(mode.scale);
        check(matches!(scale, 1 | 2), "unsupported literal desktop scale")?;
        let screen = [i64::from(mode.width),i64::from(mode.height)];
        let reader = Reader::attach()?;
        // Attaching is passive: a real pointer event wakes bwm after attachment.
        session.raw_motion(i32::MIN/2, i32::MIN/2)?;
        wait(&reader, [lower_id,upper_id], [lower,upper], scale, screen, false)?;
        // Center of upper window's minimize button, from literal frame/chrome sizes.
        session.raw_motion((320+44*scale) as i32, (105*scale) as i32)?;
        session.raw_press(0x110)?;
        session.raw_release(0x110)?;
        wait(&reader, [lower_id,upper_id], [lower,upper], scale, screen, true)
    })();
    let teardown = session.teardown();
    result?;
    teardown?;
    Ok(())
}
