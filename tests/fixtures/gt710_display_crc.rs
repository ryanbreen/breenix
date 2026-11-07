extern crate alloc;
extern crate self as gpu_abi;
#[path = "ABI/screen_crc.rs"] pub mod screen_crc;
#[path = "ABI/screen.rs"] mod screen;
// These complete production modules also supply existing timing helpers that
// this focused fixture does not call.
#[allow(dead_code)]
#[path = "DRIVER/edid.rs"] mod edid;
#[allow(dead_code)]
#[path = "DRIVER/modeset.rs"] mod modeset;
#[path = "DRIVER/display_crc.rs"] pub mod display_crc;
use display_crc::{Binding, Checkpoint, Io, Sample};
use std::collections::BTreeMap;

const PIN: Binding = Binding { resource: 7, present_fence: 9 };
fn checkpoint() -> Checkpoint<'static> {
    Checkpoint { suite: "gpu", case: "present/example", checkpoint: "end" }
}
struct Model {
    now: u64,
    methods: BTreeMap<u32, u32>,
    submitted: Vec<Vec<(u32,u32)>>,
    available: Vec<Sample>,
    reads: Vec<usize>,
    fault: u8,
    phase: u8,
    recovered: usize,
    changed: bool,
    status: u32,
    error: u32,
}
impl Model {
    fn new() -> Self {
        let t = edid::Timing { width:1280,height:1024,clock_khz:108000,
            hsync_start:1328,hsync_end:1440,htotal:1688,
            vsync_start:1025,vsync_end:1028,vtotal:1066,flags:30 };
        let methods = modeset::logical_head_methods(&t,0,0,1,0x08000000,10240,2560,1600)
            .unwrap().into_iter().collect();
        Self { now:0,methods,submitted:vec![],available:vec![sample(91,92),
            sample(11,12),sample(11,12),sample(11,12),sample(11,12)],
            reads:vec![],fault:0,phase:0,recovered:0,changed:false,status:1,error:0 }
    }
    fn capture(&mut self) -> Result<display_crc::Record, &'static str> {
        display_crc::capture(self, checkpoint(), PIN, [2560,1600], 0, 0, 4)
    }
    fn disarmed(&self) {
        assert_eq!(self.submitted.last().unwrap(),
            &[(0x430,0xffffff40),(0x438,0),(0x84,0x80000000),(0x80,0)]);
    }
}
fn sample(crc:u32,crc2:u32)->Sample { Sample {status:1,crc,crc2} }
impl modeset::Io for Model {
    fn read(&mut self, reg:usize)->u32 {
        let method=(reg&0xffff) as u32;
        let value=*self.methods.get(&method).unwrap_or(&0);
        if self.fault==1 && reg==0x6604a0 {value^1} else {value}
    }
    fn write(&mut self, _:usize, _:u32) {panic!("collector bypassed the core channel")}
    fn delay_us(&mut self, us:u32) {self.now+=u64::from(us);}
}
impl Io for Model {
    fn now_us(&self)->u64 {self.now}
    fn binding(&mut self)->Result<Binding,&'static str> {
        if self.fault==2 {return Err("binding unavailable")}
        if self.changed && self.phase>=1 {Ok(Binding{present_fence:10,..PIN})} else {Ok(PIN)}
    }
    fn clear(&mut self)->Result<(),&'static str> {
        if self.fault==3 {Err("clear failed")} else {Ok(())}
    }
    fn update(&mut self, methods:&[(u32,u32)], deadline:u64)->Result<(),&'static str> {
        assert!(deadline>=self.now);
        self.submitted.push(methods.to_vec());
        self.phase+=1;
        self.now+=1000;
        if self.fault==4 && self.phase==1 {return Err("arm failed after publication")}
        if self.fault==5 && self.phase==2 {return Err("disarm failed")}
        Ok(())
    }
    fn context_status(&mut self)->Result<u32,&'static str> {
        if self.fault==6 {Err("status unreadable")} else if self.phase<2 {Ok(self.status&!1)} else {Ok(self.status)}
    }
    fn sample(&mut self, index:usize)->Result<Option<Sample>,&'static str> {
        self.reads.push(index);
        if self.phase<2 {self.now+=16000;}
        if self.fault==7 {return Err("notifier read failed")}
        let sample = self.available.get(index).copied();
        if self.fault == 8 && self.phase < 2 {
            return Ok(sample.map(|s| Sample { crc2: 0, ..s }));
        }
        Ok(sample)
    }
    fn display_error(&mut self)->u32 {self.error}
    fn recover_present(&mut self)->Result<(),&'static str> {self.recovered+=1;Ok(())}
}
#[test] fn core_encodings_and_order_are_literal() {
    let controls=[0xff8f0f40,0xff9f1f40,0xffaf2f40,0xffbf3f40];
    for h in 0..4 {
        assert_eq!(modeset::crc_methods(h,h,4,true).unwrap(),[
            (0x438+h*0x300,4),(0x430+h*0x300,controls[h as usize]),
            (0x84,0x80000000),(0x80,0)]);
        assert_eq!(modeset::crc_methods(h,h,4,false).unwrap(),[
            (0x430+h*0x300,0xffffff40),(0x438+h*0x300,0),
            (0x84,0x80000000),(0x80,0)]);
    }
    assert!(modeset::crc_methods(4,0,4,true).is_err());
    assert!(modeset::crc_methods(0,4,4,true).is_err());
    assert!(modeset::crc_methods(0,0,0,true).is_err());
}
#[test] fn discard_first_and_capture_four_consecutive_nonzero_entries() {
    let mut m=Model::new();let r=m.capture().unwrap();m.disarmed();
    assert_eq!(m.submitted[0],[(0x438,4),(0x430,0xff8f0f40),(0x84,0x80000000),(0x80,0)]);
    assert_eq!(r.samples,m.available);assert_eq!(m.reads,[0,1,2,3,4,0,1,2,3,4,5]);
    assert_eq!(r.binding_before,PIN);assert_eq!(r.binding_after,PIN);
    assert!(r.disarmed);assert_eq!(r.status,1);assert_eq!(r.display_error,0);
    assert!(r.elapsed_us<=250000);assert_eq!(m.recovered,0);
    assert_eq!(r.output_config.mode,[2560,1600]);
    assert_eq!(r.output_config.raster,[1280,1024]);
    assert_eq!(r.output_config.viewport,[0,112,1280,800]);
    let methods=&r.output_config.methods;
    assert!(methods.windows(2).all(|w|w[0].0<w[1].0));
    assert!(methods.iter().all(|(a,v)|a&3==0 && m.methods[a]==*v));
    for a in [0x200,0x404,0x414,0x498,0x4a0,0x4b8,0x4c0,0x4c8] {
        assert!(methods.iter().any(|(r,_)|*r==a));
    }
}
#[test] fn not_written_is_retried_in_place_and_cannot_fill_the_sample_quorum() {
    let mut m=Model::new();m.available.truncate(4);
    assert!(m.capture().is_err());m.disarmed();
    assert!(m.reads.iter().filter(|&&i|i==4).count()>1);
    let mut m=Model::new();m.available[2]=sample(0,12);
    assert!(m.capture().is_err());m.disarmed();
    assert!(!m.reads.contains(&3));
    let mut m=Model::new();m.available[2]=sample(11,0);
    assert_eq!(m.capture().unwrap_err(), "CRC completed notifier tail is unstable or partial");
    m.disarmed();
    assert_eq!(m.reads, [0,1,2,3,4,0,1,2]);
}
#[test] fn unstable_output_overflow_binding_move_and_display_errors_refuse() {
    for second in [false,true] {
        let mut m=Model::new();m.available[3]=if second {sample(11,13)} else {sample(13,12)};
        assert!(m.capture().is_err());m.disarmed();
    }
    for status in [0,2,4,8,16,32,63] {
        let mut m=Model::new();m.status=status;
        assert!(m.capture().is_err());m.disarmed();
    }
    let mut m=Model::new();m.changed=true;
    assert!(m.capture().is_err());m.disarmed();
    let mut m=Model::new();m.error=4;
    assert!(m.capture().is_err());m.disarmed();
}
#[test] fn exit_paths_disarm_and_failed_arm_uses_present_recovery() {
    for fault in 1..=7 {
        let mut m=Model::new();m.fault=fault;
        assert!(m.capture().is_err(),"fault={fault}");m.disarmed();
        assert_eq!(m.recovered,usize::from(fault==4||fault==5),"fault={fault}");
    }
}
#[test] fn notifier_and_time_bounds_are_not_extended() {
    let mut m=Model::new();m.now=1_000_000;
    let r=m.capture().unwrap();assert!(r.elapsed_us<=250000);
    let mut m=Model::new();m.available=vec![sample(11,12);255];
    // Collection stops at K=4, then drains the completed notifier extent.
    let r=m.capture().unwrap();assert_eq!(r.samples.len(),255);
    assert_eq!(display_crc::MAX_SAMPLES,255);
    assert_eq!(display_crc::TIMEOUT_US,250000);
    assert_eq!(display_crc::notifier_offset(254).unwrap(),4072);
    assert!(display_crc::notifier_offset(255).is_err());
}
#[test] fn binary_record_carries_sidecar_values_without_struct_padding() {
    let mut m=Model::new();let mut r=m.capture().unwrap();
    // Test the upper record size with the notifier's complete written extent.
    r.samples=vec![sample(0x12345678,0xabcdef12);255];
    let bytes=r.encode().unwrap();assert!(bytes.len()<4096);
    assert_eq!(&bytes[..8],b"SCRCRC01");
    let u32at=|i|u32::from_le_bytes(bytes[i..i+4].try_into().unwrap());
    let u64at=|i|u64::from_le_bytes(bytes[i..i+8].try_into().unwrap());
    assert_eq!(u32at(8),1);assert_eq!(u32at(12) as usize,bytes.len());
    assert_eq!(u32at(16),6);
    assert_eq!(u64at(32),7);assert_eq!(u64at(40),9);
    assert_eq!(u64at(48),7);assert_eq!(u64at(56),9);
    assert_eq!(u32at(64),r.elapsed_us);assert_eq!(u32at(68),0);assert_eq!(u32at(72),1);
    assert_eq!([u32at(76),u32at(80),u32at(84),u32at(88)],[2560,1600,1280,1024]);
    assert_eq!([u32at(92),u32at(96),u32at(100),u32at(104)],[0,112,1280,800]);
    assert_eq!([u32at(108),u32at(112)],[0,0]);
    let names=b"gpupresent/exampleend";
    assert_eq!(&bytes[116..116+names.len()],names);
    let mut at=116+names.len();
    for &(method,value) in &r.output_config.methods {
        assert_eq!(u32at(at),method);assert_eq!(u32at(at+4),value);at+=8;
    }
    for _ in 0..255 {assert_eq!([u32at(at),u32at(at+4),u32at(at+8)],[1,0x12345678,0xabcdef12]);at+=12;}
    assert_eq!(at,bytes.len());
    r.samples.push(sample(1,2));assert!(r.encode().is_err());
}

#[test] fn entries_written_before_disarm_are_retained_and_validated() {
    let mut m=Model::new();m.available.push(sample(11,12));
    let r=m.capture().unwrap();assert_eq!(r.samples,m.available);m.disarmed();
    for tail in [sample(13,12),sample(11,13),sample(11,0)] {
        let mut m=Model::new();m.available.push(tail);
        assert!(m.capture().is_err());m.disarmed();
    }
}

#[test]
fn crc_codec_and_request_roundtrip() {
    let record = Model::new().capture().unwrap();
    let bytes = record.encode().unwrap();
    let decoded = screen_crc::Record::decode(&bytes).unwrap();
    assert_eq!(decoded.encode().unwrap(), bytes);
    assert_eq!(decoded.suite, "gpu");
    assert_eq!(decoded.case, "present/example");
    assert_eq!(decoded.checkpoint, "end");
    assert_eq!(decoded.binding_before, PIN);
    assert_eq!(decoded.binding_after, PIN);
    assert_eq!(decoded.samples, record.samples);
    assert_eq!(decoded.output_config.methods, record.output_config.methods);
    for n in 0..bytes.len() { assert!(screen_crc::Record::decode(&bytes[..n]).is_err()); }
    let mut bad = bytes.clone(); bad.push(0);
    assert!(screen_crc::Record::decode(&bad).is_err());
    for at in [0, 8, 12, 16, 26, 28, 30] {
        let mut bad = bytes.clone(); bad[at] ^= 0x80;
        assert!(screen_crc::Record::decode(&bad).is_err(), "accepted offset {at}");
    }
    let request = screen::CrcRequest { pixels: 0x1000, len: 4096, observation: 0x2000,
        record: 0x3000, suite: "gpu", case: "present/example", checkpoint: "end" };
    let encoded = request.encode().unwrap();
    let decoded = screen::CrcRequest::decode(&encoded).unwrap();
    assert_eq!(decoded, request);
    assert!(screen::Request::decode(&encoded).is_none());
    for n in 0..encoded.len() { assert!(screen::CrcRequest::decode(&encoded[..n]).is_none()); }
    let mut bad = encoded.clone(); bad.push(0);
    assert!(screen::CrcRequest::decode(&bad).is_none());
    let mut bad = encoded.clone(); bad[46] = 1;
    assert!(screen::CrcRequest::decode(&bad).is_none());
    let pixel = screen::Request {pixels:1,len:2,record:3}.encode();
    assert_eq!(screen::rectangle([2,1],[4,2],Some([0,0])),Some([0,0,2,1]));
    assert_eq!(screen::NO_CASE,61);
    let failure=screen::Failure{errno:5,reason:4,failed_submission:9,declared:1}.encode();
    assert_eq!(screen::Failure::decode(&failure).unwrap().failed_submission,9);
    assert_eq!(pixel.len(), 32);
    assert_eq!(&pixel[..8], b"SCREENRB");
    let decoded = screen::Request::decode(&pixel).unwrap();
    assert_eq!((decoded.pixels, decoded.len, decoded.record), (1,2,3));
    let mut invalid = request;
    invalid.case = "bad case";
    assert!(invalid.encode().is_err());
}

#[test] fn failed_observation_preserves_driver_reason_and_raw_state() {
    let mut m = Model::new();
    m.fault = 1;
    let error = m.capture().unwrap_err();
    let detail = display_crc::failure_snapshot(&mut m, error, 0, 0);
    assert!(detail.contains("error=CRC_output_is_not_latched"), "{detail}");
    assert!(detail.contains("method=000004a0"), "{detail}");
    assert!(detail.contains("active=00000000 assembly=00000001"), "{detail}");
    assert!(detail.contains("sample0=0000005b:0000005c"), "{detail}");
    assert!(detail.contains("status=00000000"), "{detail}");
}

#[test] fn secondary_words_are_judged_after_context_done() {
    let mut m = Model::new();
    m.fault = 8;
    let record = m.capture().expect("secondary writes become valid at completion");
    m.disarmed();
    assert_eq!(record.samples, m.available);
    assert!(record.samples.iter().all(|s| s.crc != 0 && s.crc2 != 0));
    assert_eq!(record.status, 1);
}

#[test] fn missing_secondary_probe_selects_rg_as_primary_without_approval() {
    let mut m = Model::new();
    m.available = vec![sample(33,0); 6];
    let detail = display_crc::probe_primary_rg(&mut m, PIN, 0, 0, 4);
    assert_eq!(m.submitted[0], [(0x438,4),(0x430,0xfffff840),(0x84,0x80000000),(0x80,0)]);
    m.disarmed();
    assert!(detail.contains("rg_primary=00000021"), "{detail}");
    assert!(detail.contains("rg_status=00000001"), "{detail}");
    // An observation with a missing simultaneous secondary still fails.
    let mut m = Model::new();
    m.available = vec![sample(33,0); 6];
    assert!(m.capture().is_err());
}
