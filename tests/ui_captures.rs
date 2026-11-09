#[path = "../libs/libuitest/src/messages.rs"] pub mod messages;
extern crate self as libbreenix;
extern crate self as libgfx;
#[path = "../libs/libuitest/src/arena.rs"] pub mod arena;
#[path = "../libs/libuitest/src/capture_marker.rs"] pub mod capture_marker;
#[path = "../libs/libuitest/src/capture_race.rs"] pub mod capture_race;
#[path = "../libs/libuitest/src/capture.rs"] pub mod capture;
#[path = "../libs/libuitest/src/compare.rs"] pub mod compare;
#[path = "../libs/libuitest/src/schema.rs"] pub mod schema;
#[path = "../libs/libuitest/src/outcome.rs"] pub mod outcome;
pub use outcome::{FailureClass, ScenarioOutcome};
#[path = "../libs/libbreenix/src/window.rs"] pub mod window;
#[derive(Debug)] pub struct DeviceError;
impl std::fmt::Display for DeviceError { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f,"device refused") } }
pub mod error { pub use crate::DeviceError as Error; impl Error { pub fn from_syscall(value: i64) -> Result<u64, Self> { if value >= 0 { Ok(value as u64) } else { Err(Self) } } } }
pub mod gpu {
    pub use gpu_abi as abi;
    pub struct Mode { pub width:u32, pub height:u32 }
    pub fn current_mode() -> Result<Mode, crate::DeviceError> { Ok(Mode { width:16, height:4 }) }
}
pub mod surface {
    pub struct Surface;
    impl Surface {
        pub fn width(&self)->usize {16}
        pub fn height(&self)->usize {4}
        pub fn format(&self)->Format { Format::BGRX }
        pub fn read(&self, out: &mut [u8], stride:usize)->Result<(),i64> {
            assert_eq!(stride,64);
            out[..256].copy_from_slice(&crate::capture_marker::frame(16,4,crate::capture_marker::Generation {commit:7,producer:8}).unwrap()); Ok(())
        }
    }
    #[derive(PartialEq,Eq)] pub struct Format;
    impl Format { pub const BGRX:Self=Self; }
}
pub mod syscall { pub mod raw {
    pub unsafe fn syscall3(_: u64, op: u64, ptr: u64, _: u64) -> u64 {
        let mut state=crate::DEVICE.lock().unwrap();
        if state.refuse { return (-5i64) as u64; }
        if op == breengel_abi::op::CAPTURE as u64 {
            let c=&mut *(ptr as *mut breengel_abi::Capture);
            assert!(c.tree_dst!=0,"tree must accompany pixels in one call");
            let g=crate::capture_marker::Generation {commit:7,producer:8};
            let pixels=crate::capture_marker::frame(16,4,g).unwrap();
            let tree=crate::capture::root(g,16,4);
            std::ptr::copy_nonoverlapping(pixels.as_ptr(),c.dst as *mut u8,pixels.len());
            std::ptr::copy_nonoverlapping(tree.as_ptr(),c.tree_dst as *mut u8,tree.len());
            c.commit_id=7;c.width=16;c.height=4;state.calls+=1;
        } else if op == breengel_abi::op::SCANOUT_READBACK as u64 {
            let r=&mut *(ptr as *mut breengel_abi::Readback);
            let bytes=std::slice::from_raw_parts_mut(r.record as *mut u8,1024);
            let word=|b:&mut [u8], at:usize,v:u32| b[at..at+4].copy_from_slice(&v.to_le_bytes());
            let quad=|b:&mut [u8], at:usize,v:u64| b[at..at+8].copy_from_slice(&v.to_le_bytes());
            for (at,v) in [(0,2),(4,1024),(16,6),(20,5),(24,3),(40,16),(44,4),(56,64),(112,0x60000000),(116,0xa1971b0c),(128,0x60000000),(132,0xa0b50300),(152,0xa0b50300),(156,0x60000000),(192,0x60000000),(196,2)] {word(bytes,at,v);}
            for (at,v) in [(8,91),(80,1),(104,2),(120,3),(176,90),(184,4),(200,11),(208,7),(216,70),(224,3),(232,12),(240,9),(248,90),(256,4)] {quad(bytes,at,v);}
            if state.invalid {word(bytes,164,1);}
            std::slice::from_raw_parts_mut(r.dst as *mut u8,256).fill(0x42);
            r.serial=91;state.calls+=1;
        } else { panic!("unexpected op {op}"); }
        0
    }
} }
#[derive(Default)] struct State {calls:usize,refuse:bool,invalid:bool}
static DEVICE:std::sync::Mutex<State>=std::sync::Mutex::new(State {calls:0,refuse:false,invalid:false});
static SERIAL:std::sync::Mutex<()>=std::sync::Mutex::new(());
fn arena(name:&str)->(std::path::PathBuf,arena::Arena) {
    let path=std::env::temp_dir().join(format!("ui-capture-{}-{name}",std::process::id()));
    let arena=arena::Arena::create(&path,arena::Layout::for_mode(16,4).unwrap()).unwrap();(path,arena)
}
#[test] fn window_publishes_same_call_pixels_and_root() {
    let _guard=SERIAL.lock().unwrap();*DEVICE.lock().unwrap()=State::default();
    let (path,mut arena)=arena("window");
    let record=capture::window(&mut arena,"window",11).unwrap();
    assert_eq!(DEVICE.lock().unwrap().calls,1);
    let arena::SlotRead::Final {bytes:pixels,..}=arena.read(arena::Slot::Frame).unwrap() else {panic!()};
    let arena::SlotRead::Final {bytes:tree,record:meta}=arena.read(arena::Slot::Tree).unwrap() else {panic!()};
    assert_eq!(meta.commit,7);assert_eq!(record.window,11);
    let marker=capture::root_marker(&tree).unwrap();
    assert!(capture_marker::whole(&pixels,64,4,&marker,record.commit));
    let value:serde_json::Value=serde_json::from_slice(tree.split(|&b|b==0).next().unwrap()).unwrap();
    assert_eq!(value["value"]["commit"],7);assert_eq!(value["value"]["producer"],8);
    let mut altered=tree; let at=altered.iter().position(|&b| b==b'7').unwrap();altered[at]=b'6';
    assert!(!capture_marker::whole(&pixels,64,4,&capture::root_marker(&altered).unwrap(),7));
    drop(arena);std::fs::remove_file(path).unwrap();
}
#[test] fn screen_uses_shared_validator_and_keeps_commit_list() {
    let _guard=SERIAL.lock().unwrap();*DEVICE.lock().unwrap()=State::default();
    let(path,mut a)=arena("screen");let r=capture::screen(&mut a,"screen",window::ObservationKind::Composed).unwrap();
    assert_eq!(r.composite,90);assert_eq!(capture::screen_windows(&r).unwrap(),vec![(11,7),(12,9)]);
    let arena::SlotRead::Final {bytes,..}=a.read(arena::Slot::Screen).unwrap() else {panic!()};assert_eq!(bytes,vec![0x42;256]);
    let mut bad=r.clone();bad.observation[196..200].copy_from_slice(&17u32.to_le_bytes());assert!(capture::screen_windows(&bad).is_err());
    drop(a);std::fs::remove_file(path).unwrap();
    DEVICE.lock().unwrap().invalid=true;let(path,mut a)=arena("invalid");
    assert!(capture::screen(&mut a,"invalid",window::ObservationKind::Composed).is_err());
    assert!(matches!(a.read(arena::Slot::Screen).unwrap(),arena::SlotRead::Partial(_)));
    drop(a);std::fs::remove_file(path).unwrap();
}
#[test] fn failed_device_capture_stays_partial_in_both_slots() {
    let _guard=SERIAL.lock().unwrap();*DEVICE.lock().unwrap()=State {refuse:true,..Default::default()};
    let(path,mut a)=arena("failed");assert!(capture::window(&mut a,"window",11).is_err());
    for slot in [arena::Slot::Frame,arena::Slot::Tree] {assert!(matches!(a.read(slot).unwrap(),arena::SlotRead::Partial(_)));}
    drop(a);std::fs::remove_file(path).unwrap();
}
#[test] fn surface_capture_and_planted_outcome() {
    let(path,mut a)=arena("surface");capture::surface(&mut a,"surface",&surface::Surface).unwrap();
    let arena::SlotRead::Final {bytes:want,record}=a.read(arena::Slot::Frame).unwrap() else {panic!()};assert_eq!(record.commit,0);
    let mut got=want.clone();got[(2*16+12)*4]^=1;
    assert!(matches!(capture::compare_outcome("marker",&want,&want,16,4).unwrap(),ScenarioOutcome::Pass {..}));
    assert!(matches!(capture::compare_outcome("marker",&want,&got,16,4).unwrap(),ScenarioOutcome::Fail {class:FailureClass::PixelMismatch,operation,evidence,..} if operation=="marker" && evidence=="first=12,2 got=#182031 want=#182030"));
    drop(a);std::fs::remove_file(path).unwrap();
}

#[test]
fn race_judge_requires_fired_rise_and_one_generation() {
    let g = capture_marker::Generation { commit: 7, producer: 8 };
    let pixels = capture_marker::frame(16, 4, g).unwrap();
    let tree = capture::root(g, 16, 4);
    let judge = |before, after, px: &[u8], tr: &[u8], commit, expected| {
        capture_race::judge(before, after, px, 16, 4, tr, commit, expected)
    };
    assert_eq!(judge(40, 41, &pixels, &tree, 7, Some(g)).unwrap(), g);
    assert!(judge(40, 40, &pixels, &tree, 7, Some(g)).unwrap_err().contains("did not fire"));
    assert!(judge(40, 39, &pixels, &tree, 7, Some(g)).is_err());
    let next = capture_marker::Generation { commit: 8, producer: 9 };
    let mut mixed = pixels.clone();
    mixed[64..96].copy_from_slice(&capture_marker::encode(next));
    assert!(judge(40, 41, &mixed, &tree, 7, None).is_err());
    assert!(judge(40, 41, &pixels, &capture::root(next,16,4), 7, None).is_err());
    assert!(judge(40, 41, &pixels, &tree, 8, None).is_err());
    assert!(judge(40, 41, &pixels, &tree, 7, Some(next)).is_err());
    let mut stray = pixels.clone(); stray[60] ^= 1;
    assert!(judge(40, 41, &stray, &tree, 7, None).is_err());
}

#[test]
fn typed_capture_error_distinguishes_device_from_arena_refusal() {
    let _guard = SERIAL.lock().unwrap();
    *DEVICE.lock().unwrap() = State { refuse: true, ..Default::default() };
    let (path, mut arena) = arena("typed-error");
    let device = capture::window_result(&mut arena, "race", 11).unwrap_err();
    assert!(device.syscall.is_some());
    assert!(device.detail.contains("window capture 11:"));
    let slot = capture::window_result(&mut arena, "race", 11).unwrap_err();
    assert!(slot.syscall.is_none());
    assert_eq!(slot.detail, "arena slot already used");
    drop(arena); std::fs::remove_file(path).unwrap();
}

#[test] fn repeated_screen_observations_use_fresh_one_shot_arenas() {
    let _guard=SERIAL.lock().unwrap();
    *DEVICE.lock().unwrap()=State::default();
    let path=std::env::temp_dir().join(format!("ui-screen-scratch-{}",std::process::id()));
    let layout=arena::Layout::for_mode(16,4).unwrap();
    for _ in 0..3 {
        let (record,bytes)=capture::screen_fresh(&path,layout,"cursor",window::ObservationKind::Composed).unwrap();
        assert_eq!(record.composite,90);
        assert_eq!(bytes,vec![0x42;256]);
        assert!(!path.exists());
    }
    assert_eq!(DEVICE.lock().unwrap().calls,3);
}

#[test] fn old_cursor_sequence_refuses_second_screen_capture() {
    let _guard=SERIAL.lock().unwrap();
    *DEVICE.lock().unwrap()=State::default();
    let (path,mut arena)=arena("cursor-old");
    capture::screen(&mut arena,"before-motion",window::ObservationKind::Composed).unwrap();
    assert_eq!(capture::screen(&mut arena,"cursor-screen",window::ObservationKind::Composed).unwrap_err(),"arena slot already used");
    drop(arena);std::fs::remove_file(path).unwrap();
}

#[test]
fn overlap_witness_records_preserve_control_deltas() {
    let mut records = Vec::new();
    capture_race::record_witness(40, 41, 70, 71, |label, value| records.push((label, value))).unwrap();
    assert_eq!(records, vec![("capture-stall-fired", 1), ("capture-release-fired", 1)]);
    for (before, fired, release_before, released) in [(40,40,70,71),(40,39,70,71),(40,41,70,70),(40,41,70,69)] {
        let mut records = Vec::new();
        assert!(capture_race::record_witness(before, fired, release_before, released,
            |label, value| records.push((label, value))).is_err());
        assert!(records.is_empty());
    }
}
