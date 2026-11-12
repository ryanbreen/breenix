use std::cell::RefCell;
struct Screen { pixels: Vec<u8>, record: [u8; 1024], width: u32, height: u32 }
impl Screen { @SCREEN@ }
#[derive(Default)]
struct WaitObservation { error: Option<String>, record: Option<[u8; 1024]>, pixel: Option<u32> }
@OBSERVATION@
@QUAD@
@DIAGNOSTIC@
fn word(b: &[u8], p: usize) -> u32 { u32::from_le_bytes(b[p..p+4].try_into().unwrap()) }
mod gpu {
    pub struct Mode { pub width:u32, pub height:u32 }
    pub fn current_mode()->Result<Mode,String> { Ok(Mode { width:2, height:2 }) }
}
mod abi {
    pub struct Readback { pub dst:u64, pub dst_len:u64, pub record:u64, pub record_len:u32, pub reason:u32 }
    impl Readback { pub const ZERO: Self = Self { dst:0, dst_len:0, record:0, record_len:0, reason:0 }; }
}
mod window {
    #[derive(Clone, Copy, PartialEq, Eq)]
    @VALIDATE@
    pub fn readback(r:&mut crate::abi::Readback)->Result<(),String> {
        crate::READS.with(|q| {
            let screen = q.borrow_mut().pop_front().expect("unexpected readback")?;
            assert_eq!(r.record_len,1024); assert_eq!(r.dst_len,16);
            // The production reader allocated both buffers for this call.
            unsafe {
                std::ptr::copy_nonoverlapping(screen.record.as_ptr(),r.record as *mut u8,1024);
                std::ptr::copy_nonoverlapping(screen.pixels.as_ptr(),r.dst as *mut u8,16);
            }
            Ok(())
        })
    }
}
thread_local! { static READS: RefCell<std::collections::VecDeque<Result<Screen,String>>> = RefCell::new(Default::default()); }
fn ack(before:Result<Screen,String>, after:Screen)->Result<bool,String> {
    READS.with(|q| *q.borrow_mut()=[before,Ok(after)].into());
    @BASELINE@
    let mut observation=WaitObservation::default();
    let d=gpu::current_mode()?;
    @ACK@
}
fn record(backend:u32, context:u64, serial:u64, flags:u32, pixel:u32)->Screen {
    let mut s=Screen { record:[0;1024], pixels:pixel.to_le_bytes().repeat(4),width:2,height:2 };
    let (boundary,status,render,read,copy)=match backend {
        6 => (5,0x60000000,0xa1971b0c,0xa0b50300,0xa0b50300),
        4 => (4,0x1100,0x207,0x206,0),
        _ => panic!("unsupported fixture backend"),
    };
    for (at,v) in [(0,2),(4,1024),(16,backend),(20,boundary),(24,flags),(40,2),(44,2),(56,8),
        (112,status),(116,render),(128,status),(132,read),(152,copy),
        (156,if backend==6 {status}else{0}),(192,status)] {
        s.record[at..at+4].copy_from_slice(&v.to_le_bytes());
    }
    for (at,v) in [(64,context),(80,9),(104,17),(120,18),(176,serial),(184,19)] {
        s.record[at..at+8].copy_from_slice(&v.to_le_bytes());
    }
    s
}
#[test]
fn launch_requires_a_new_completed_composite_not_a_different_context() {
    // Native readback names the same channel for console and bwm.
    assert!(ack(Ok(record(6,1,0,1,0)),record(6,1,1,3,0xff14161e)).unwrap(),
        "GK208 console and bwm share context 1; the new completed composite must acknowledge");
    // Virtio names distinct device contexts; the same freshness rule applies.
    assert!(ack(Ok(record(4,1,0,1,0)),record(4,8,1,3,0xff14161e)).unwrap());
    // A retained composite baseline must be advanced even if context changes.
    for serial in [0,6,7] {
        assert!(!ack(Ok(record(6,1,7,3,0xff14161e)),record(6,2,serial,3,0xff14161e)).unwrap());
    }
    assert!(ack(Ok(record(6,1,7,3,0xff14161e)),record(6,1,8,3,0xff14161e)).unwrap());
    assert!(!ack(Ok(record(6,1,0,1,0)),record(6,2,1,1,0xff14161e)).unwrap());
    assert!(!ack(Ok(record(6,1,0,1,0)),record(6,2,1,3,0xff000000)).unwrap());
    assert!(ack(Err("baseline unavailable".into()),record(6,1,1,3,0xff14161e)).is_err());
    let mut invalid=record(6,1,0,1,0); invalid.record[128..132].fill(0);
    assert!(ack(Ok(invalid),record(6,1,1,3,0xff14161e)).is_err());
}
#[test]
fn completed_observation_survives_failure_record_truncation() {
    let s=record(6,1,42,3,0xff14161e);
    let observation=WaitObservation { record:Some(s.record), pixel:Some(0xff14161e),
        error:Some("early plain scanout refusal ".repeat(100)) };
    let detail=observation.failure();
    failure_diagnostic("ready",&detail);
    let output=std::fs::read_to_string(@EVIDENCE@).unwrap();
    let first=output.lines().next().unwrap();
    for value in ["last completed", "context=1", "serial=42", "flags=3", "pixel=Some(4279506462)"] {
        assert!(first.contains(value),"missing {value} in first bounded diagnostic: {first}");
    }
    assert!(detail.find("last completed").unwrap()<detail.find("last readback error").unwrap());
    for _ in 0..100 { failure_diagnostic("ready",&detail); }
    assert!(std::fs::read(@EVIDENCE@).unwrap().len()<=1024);
}
