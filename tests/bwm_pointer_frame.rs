//! Compile bwm's production Stream, source accounting and router without guest I/O.
//! Run directly: rustc --edition=2021 --test tests/bwm_pointer_frame.rs -o /tmp/bwm-pointer-frame
use std::{fs, process::Command};

#[test]
fn pointer_frames_survive_read_boundaries_and_account_for_every_source() {
    let root = std::path::Path::new(env!("PWD"));
    let source = include_str!("../userspace/programs/src/bwm/input.rs");
    let stream = source.split_once("pub(super) struct Stream").unwrap().1
        .split_once("pub(super) fn send_repeat").unwrap().0;
    let context = source.split_once("static SOURCE:").unwrap().1
        .split_once("// Published only").unwrap().0;
    let begin_finish = source.split_once("pub(super) fn begin(").unwrap().1
        .split_once("fn losses(").unwrap().0;
    let send = source.split_once("pub(super) fn send_id(").unwrap().1
        .split_once("pub(super) fn send_keys(").unwrap().0;
    let keys = include_str!("../userspace/programs/src/bwm/input_keys.rs");
    let normalize = keys.split_once("pub(super) fn normalize(").unwrap().1
        .split_once("pub(super) fn events(").unwrap().0;
    let input_abi = include_str!("../libs/input-abi/src/lib.rs").replace("#![no_std]", "");
    let fixture = format!(r#"
#[allow(dead_code)]
mod fixture {{
#[allow(dead_code)]
pub mod input_abi {{ {input_abi} }}
#[allow(dead_code)]
#[path = "{root}/userspace/programs/src/bwm/stack.rs"] mod stack;
#[allow(dead_code)]
#[path = "{root}/userspace/programs/src/bwm/pointer_route.rs"] mod pointer_route;
mod input_keys {{
    use super::input_abi::Capability;
    pub(super) fn normalize({normalize}
}}
// Only the reader/transport is replaced; normalization, frame state, routing
// and serial/disposition decisions below are the unmodified production code.
#[allow(dead_code)]
mod core_input {{
    pub use super::input_abi::*;
    pub use super::input_abi as abi;
    pub static RECORDS: std::sync::Mutex<Vec<Record>> = std::sync::Mutex::new(Vec::new());
    pub struct Reader;
    impl Reader {{
        pub fn open(_: bool) -> Result<Self, ()> {{ Ok(Self) }}
        pub fn grab(&self, _: &[u32]) -> Result<(), ()> {{ Ok(()) }}
        pub fn capabilities(&self, _: u32, caps: &mut [Capability]) -> Result<usize, super::libbreenix::error::Error> {{
            caps[0] = Capability {{ kind: kind::ABS, code: axis::X, minimum: 0, maximum: 199 }};
            caps[1] = Capability {{ kind: kind::ABS, code: axis::Y, minimum: 0, maximum: 199 }};
            Ok(2)
        }}
        pub fn fd(&self) -> Fd {{ Fd }}
        pub fn read(&self, _: &mut [u8]) -> Result<std::vec::IntoIter<Result<Record, ()>>, super::libbreenix::error::Error> {{
            Ok(std::mem::take(&mut *RECORDS.lock().unwrap()).into_iter().map(Ok).collect::<Vec<_>>().into_iter())
        }}
    }}
    pub struct Fd;
    impl Fd {{ pub fn raw(&self) -> u64 {{ 0 }} }}
    pub struct Record {{ pub event: Event, pub payload: &'static [u8] }}
    pub fn input_device_selection(_: Option<&str>) -> Result<Option<Vec<u32>>, ()> {{ Ok(None) }}
}}
mod libbreenix {{
    pub mod errno {{ #[derive(Debug)] pub enum Errno {{ ENODEV, EAGAIN, EIO }} }}
    pub mod error {{ #[derive(Debug)] pub enum Error {{ Os(super::errno::Errno) }}
        impl std::fmt::Display for Error {{ fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{ write!(f,"{{self:?}}") }} }}
    }}
}}
mod abi {{
    #[derive(Default)] pub struct WindowEvent {{ pub serial: u64, pub time_ns: u64, pub modifiers: u32 }}
}}
#[derive(Clone, Copy)] pub(super) enum Input {{ Move {{ x: i32, y: i32 }}, Button {{ x: i32, y: i32 }}, Scroll, Enter }}
mod session {{
    pub fn event(input: super::Input) -> super::abi::WindowEvent {{
        if let super::Input::Move {{ x, y }} = input {{ super::POINTS.lock().unwrap().push((x,y)); }}
        super::abi::WindowEvent::default()
    }}
}}
use std::collections::BTreeMap;
use core_input::{{axis, key, kind, syn, Event, disposition as outcome, reason}};
static READER: std::sync::OnceLock<core_input::Reader> = std::sync::OnceLock::new();
static POINTS: std::sync::Mutex<Vec<(i32,i32)>> = std::sync::Mutex::new(Vec::new());
static OUTCOMES: std::sync::Mutex<Vec<(u64,u32,u32)>> = std::sync::Mutex::new(Vec::new());
fn dispose(serial:u64, derived:u32, _:u64, disposition:u32, _:u32) {{
    OUTCOMES.lock().unwrap().push((serial,derived,disposition));
}}
struct Queued {{ event: abi::WindowEvent, source:u64, derived:u32 }}
fn enqueue(id:u64, q:Queued) {{
    assert_eq!(q.event.time_ns, q.source * 10);
    assert_eq!(q.event.modifiers, 0);
    assert_eq!(q.event.serial, if q.derived == 0 {{q.source}} else {{ core_input::abi::derived_serial(q.source,q.derived).unwrap() }});
    dispose(q.source,q.derived,id,outcome::DELIVERED,reason::NONE);
}}
static SOURCE:{context}
pub(super) fn begin({begin_finish}
pub(super) fn send_id({send}
#[allow(dead_code)]
mod production {{
    use super::*;
    pub(super) struct Stream{stream}
}}
fn event(device:u32, kind:u16, code:u16, value:i32, serial:u64) -> Event {{
    Event {{ device,kind,code,value,serial,time_ns:serial*10,flags:0 }}
}}
pub fn run() {{
    use pointer_route::{{Content, NoticeKind, Router}};
    // Exercise one read and the two split points of the same frame.
    let mut stream = production::Stream::new();
    let mut stack = stack::Stack::new();
    stack.adopt(&[(1,0,false,true,false)]);
    let contents = [Content {{id:1,x:0,y:0,width:200,height:200}}];
    let mut serial = 0;
    let mut point = (0,0);
    let mut run = |batches: Vec<Vec<(u32,u16,u16,i32)>>, expected: &[(i32,i32)]| {{
        let mut router = Router::new();
        POINTS.lock().unwrap().clear(); OUTCOMES.lock().unwrap().clear();
        let first = serial+1;
        for batch in batches {{
            *core_input::RECORDS.lock().unwrap() = batch.into_iter().map(|(d,k,c,v)| {{
                serial+=1; core_input::Record {{ event:event(d,k,c,v,serial),payload:&[] }}
            }}).collect();
            let records=stream.read();
            let positions = stream.positions(&records,200,200);
            assert_eq!(positions.len(),records.len(),"retain every source slot");
            for ((event,_),position) in records.into_iter().zip(positions) {{
                begin(event);
                if let Some(next) = stream.pointer(event,position) {{
                    let action = if event.kind == kind::KEY {{ pointer_route::Action::Button {{ code:event.code,value:event.value }} }}
                        else if event.kind == kind::SYN && event.code == syn::DROPPED {{ pointer_route::Action::Cancel }}
                        else {{ pointer_route::motion_action(point,next) }};
                    for notice in router.route(&mut stack,&contents,event.device,next,true,action).notices {{
                        if notice.kind == NoticeKind::Move {{ send_id(1,Input::Move {{x:notice.x,y:notice.y}}); }}
                        if matches!(notice.kind, NoticeKind::Button {{ .. }}) {{
                            assert_eq!((notice.x,notice.y),*expected.last().unwrap(),"button uses the completed frame position");
                            send_id(1,Input::Button {{ x:notice.x,y:notice.y }});
                        }}
                    }}
                    point=next;
                }}
                finish();
            }}
        }}
        assert_eq!(*POINTS.lock().unwrap(),expected,"no intermediate corner or pre-SYN motion");
        let outcomes=OUTCOMES.lock().unwrap();
        for serial in first..=serial {{
            assert_eq!(outcomes.iter().filter(|&&(s,d,_)| s==serial && d==0).count(),1,"source {{serial}} needs exactly one terminal outcome");
        }}
    }};
    let abs = |x,y| vec![(7,kind::ABS,axis::X,x),(7,kind::ABS,axis::Y,y),(7,kind::SYN,syn::REPORT,0)];
    run(vec![abs(60,50)], &[(60,50)]);
    let frame=abs(80,70);
    run(vec![frame[..1].to_vec(),frame[1..].to_vec()], &[(80,70)]);
    let frame=abs(100,90);
    run(vec![frame[..2].to_vec(),frame[2..].to_vec()], &[(100,90)]);
    // Single-axis frames retain the other committed axis.
    run(vec![vec![(7,kind::ABS,axis::X,120),(7,kind::SYN,syn::REPORT,0)]], &[(120,90)]);
    // Relative frames retain the existing immediate, batch-coalesced movement.
    run(vec![vec![(8,kind::REL,axis::X,5),(8,kind::REL,axis::Y,-10),(8,kind::SYN,syn::REPORT,0)]], &[(125,80)]);
    run(vec![vec![(8,kind::REL,axis::X,2)],vec![(8,kind::SYN,syn::REPORT,0)]], &[(127,80)]);
    // Drop clears only that device's pending axes; another device can finish.
    run(vec![vec![(7,kind::ABS,axis::X,160)],vec![(9,kind::ABS,axis::Y,110)],
        vec![(7,kind::SYN,syn::DROPPED,0),(7,kind::SYN,syn::REPORT,0)],
        vec![(9,kind::SYN,syn::REPORT,0)]], &[(127,110)]);
    run(vec![vec![(7,kind::ABS,axis::Y,130),(7,kind::SYN,syn::REPORT,0)]], &[(127,130)]);
    // A click in the axis frame must use that frame's completed position.
    run(vec![vec![(7,kind::ABS,axis::X,140)],vec![(7,kind::ABS,axis::Y,145),
        (7,kind::KEY,key::BUTTON_LEFT,1),(7,kind::SYN,syn::REPORT,0)]], &[(140,145)]);
    let _ = Input::Scroll;
}}
}}
fn main() {{ fixture::run(); }}
"#, root = root.display());
    let dir = std::env::temp_dir().join(format!("bwm-pointer-frame-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("frame.rs");
    let exe = dir.join("frame");
    fs::write(&file, fixture).unwrap();
    let built = Command::new("rustc").args(["--edition=2021", "-Dwarnings"])
        .arg(&file).arg("-o").arg(&exe).output().unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let ran = Command::new(&exe).output().unwrap();
    fs::remove_dir_all(dir).unwrap();
    assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
}
