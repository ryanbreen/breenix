//! Local failure evidence, exercising the production writer and comparison path.
#[path = "../libs/libuitest/src/messages.rs"] pub mod messages;
extern crate self as breenix_suite;
#[path = "../libs/libuitest/src/outcome.rs"] pub mod outcome;
pub use outcome::{FailureClass, ScenarioOutcome};
#[path = "../libs/libuitest/src/compare.rs"] pub mod compare;
#[path = "../libs/libuitest/src/schema.rs"] pub mod schema;
#[path = "../libs/libuitest/src/arena.rs"] pub mod arena;
#[path = "../libs/libuitest/src/artifacts.rs"] pub mod artifacts;
#[path = "../libs/libuitest/src/client.rs"] mod client;
pub use client::RecordClient;
#[path = "../libs/libuitest/src/frames.rs"] pub mod frames;
#[path = "../libs/libbreenix/src/suite_result.rs"] pub mod result;
pub mod suite {
    pub mod control {pub const HANG_MS:u64=10_000;}
    pub use crate::result;
    thread_local! { pub static AT_RECORD: std::cell::RefCell<Option<Box<dyn Fn(&str)>>> = std::cell::RefCell::new(None); }
    pub fn result_record(line: &str) -> Result<(), String> {
        AT_RECORD.with(|hook| hook.borrow().as_ref().expect("contained check wrote outer pipe")(line));
        Ok(())
    }
}
use std::{fs, path::{Path, PathBuf}};
use artifacts::{CheckWriter, Publish, SaveError};
use compare::{Frame, Format, Options, Rect, Mask, Tolerance};

fn root(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("uitest-artifacts-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&p); fs::create_dir_all(&p).unwrap(); p
}
fn frame(p: &[u8], w: usize, h: usize) -> Frame<'_> { Frame::new(p,w,h,w*4,Format::Bgrx).unwrap() }

#[test]
fn diff_marks_only_compared_differences_and_png_roundtrips() {
    let want = vec![80; 4*4*4]; let mut got = want.clone();
    for (x,y,delta) in [(0,0,1),(1,1,3),(2,1,4),(3,3,9)] { got[(y*4+x)*4] += delta; }
    let regions = [Rect {x:0,y:0,w:4,h:3}];
    let masks = [Mask {rect:Rect {x:2,y:1,w:1,h:1},reason:"not judged"}];
    let opts = Options {regions:&regions,masks:&masks,tolerance:Tolerance {channel:2,max_differing:0},reason:"codec budget"};
    let image = artifacts::diff(frame(&want,4,4),frame(&got,4,4),opts).unwrap();
    for (i,p) in image.pixels.chunks_exact(4).enumerate() {
        assert_eq!(p, if i==5 { &[255,0,255,255] } else { &[20,20,20,255] }, "pixel {i}");
    }
    let png = artifacts::png(frame(&got,4,4)).unwrap();
    assert_eq!(libimg::png::decode(&png).unwrap().pixels, frame(&got,4,4).canonical());
    let encoded = libimg_encode::encode(&image).unwrap();
    assert_eq!(libimg::png::decode(&encoded).unwrap(), image);
}

#[derive(Default)]
struct Ops { events: Vec<&'static str>, fail: Option<&'static str> }
impl Ops { fn event(&mut self,e: &'static str)->Result<(),String> { self.events.push(e); if self.fail==Some(e) {Err("injected".into())} else {Ok(())} } }
impl Publish for Ops {
    fn write_temp(&mut self,_:&str,_:&[u8])->Result<(),String>{self.event("write")}
    fn sync_temp(&mut self)->Result<(),String>{self.event("sync-file")}
    fn rename(&mut self,_:&str)->Result<(),String>{self.event("rename")}
    fn sync_directory(&mut self)->Result<(),String>{self.event("sync-directory")}
}
#[test]
fn publication_syncs_then_renames_and_stops_at_each_io_error() {
    let order = ["write","sync-file","rename","sync-directory"];
    let mut ops=Ops::default(); artifacts::publish(&mut ops,"got.png",b"png").unwrap();
    assert_eq!(ops.events,order);
    for (i,phase) in order.iter().enumerate() {
        let mut ops=Ops {fail:Some(phase),..Default::default()};
        assert!(artifacts::publish(&mut ops,"got.png",b"png").is_err());
        assert_eq!(ops.events,order[..=i]);
    }
}

#[test]
fn cap_is_shared_by_case_directories_and_append_keeps_prior_evidence() {
    let p=root("cap");
    // A sparse file reaches the real boot budget without allocating 32 MiB in the test.
    fs::File::create(p.join("earlier.bin")).unwrap().set_len(artifacts::CAP_BYTES as u64 - 3).unwrap();
    let mut a=artifacts::Store::new(&p,"ui-testing","diagnosis/one").unwrap();
    a.write("at-limit",b"123").unwrap();
    assert_eq!(CheckWriter::write(&mut a,"past-limit",b"4"),Err(SaveError::Cap));
    assert!(!p.join("ui-testing/diagnosis/one/past-limit").exists());
    fs::remove_file(p.join("earlier.bin")).unwrap();
    let mut b=artifacts::Store::open(&p,"ui-testing","diagnosis/one").unwrap();
    b.write("other",b"ok").unwrap();
    assert_eq!(fs::read(p.join("ui-testing/diagnosis/one/at-limit")).unwrap(),b"123");
    let mut c=artifacts::Store::new(&p,"ui-testing","diagnosis/two").unwrap();
    fs::File::create(p.join("earlier.bin")).unwrap().set_len(artifacts::CAP_BYTES as u64 - 5).unwrap();
    assert_eq!(CheckWriter::write(&mut c,"over",b"1"),Err(SaveError::Cap));
    fs::remove_dir_all(p).unwrap();
}

struct Writer { path: PathBuf, files: Vec<(String,Vec<u8>)>, error: Option<SaveError> }
impl CheckWriter for Writer {
    fn path(&self)->&Path {&self.path}
    fn write(&mut self,name:&str,bytes:&[u8])->Result<(),SaveError> {
        if let Some(e)=self.error {return Err(e)}
        self.files.push((name.into(),bytes.to_vec())); Ok(())
    }
}
struct Fixture { path:PathBuf, arena:arena::Arena, want:Vec<u8>, got:Vec<u8>, definition:schema::Value }
impl Fixture {
    fn new(name:&str, mismatch:bool)->Self {
        let path=root(name); let want=vec![30,20,10,255,60,50,40,255]; let mut got=want.clone();
        if mismatch {got[4]=99;}
        let png=artifacts::png(frame(&want,2,1)).unwrap(); fs::write(path.join("ref.png"),&png).unwrap();
        let definition=serde_json::json!({"version":1,"case":"diagnosis/local","check":"pixels","kind":"stored",
            "source":{"kind":"window","window":"app"},"at":{"checkpoint":"saved","requires":[]},
            "reference":{"path":"ref.png","sha256":frames::digest(&png)},"regions":[{"name":"all","x":0,"y":0,"w":2,"h":1}],
            "mask":[],"tolerance":{"channel":0,"maxDiffering":0}});
        let mut arena=arena::Arena::create(&path.join("arena"),arena::Layout::for_mode(2,1).unwrap()).unwrap();
        arena.write_frame_tree(|pixels,tree| {
            pixels[..8].copy_from_slice(&got); tree[..18].copy_from_slice(b"{\"commit\":41}\0\0\0\0\0");
            Ok((arena::Record {check:"pixels".into(),window:7,commit:41,width:2,height:1,..Default::default()},8))
        }).unwrap();
        let mut slot=arena.begin(arena::Slot::Screen).unwrap(); slot.bytes_mut()[..8].copy_from_slice(&want);
        slot.publish(arena::Record {check:"pixels".into(),width:2,height:1,observation:vec![42;1024],..Default::default()},8).unwrap();
        Self {path,arena,want,got,definition}
    }
    fn run(&self,writer:&mut dyn CheckWriter,save_all:bool)->(compare::Verdict,ScenarioOutcome) {
        self.run_mode(writer,save_all,false)
    }
    fn run_mode(&self,writer:&mut dyn CheckWriter,save_all:bool,outer:bool)->(compare::Verdict,ScenarioOutcome) {
        let mut bindings=frames::Bindings::default(); bindings.window("app"); bindings.checkpoint("saved");
        let mut client=if outer {RecordClient::outer("ui-testing","diagnosis/local",&["pixels"])}
            else {RecordClient::contained("ui-testing","diagnosis/local",&["pixels"])}.unwrap();
        let capture=frames::Capture {frame:frame(&self.got,2,1),source:"window:app",checkpoint:"saved",commit:41,epoch:0,barriers:&[]};
        let mut context=frames::ArtifactContext {arena:&self.arena,input_log:b"serial=1 disposition=processed\nserial=2 disposition=rejected\n",writer,save_all};
        let verdict=frames::check_stored_with_artifacts("ui-testing",&self.definition,capture,&bindings,&self.path,&mut client,&mut context).unwrap();
        let outcome=client.finish(ScenarioOutcome::pass()); (verdict,outcome)
    }
}
#[test]
fn mismatch_publishes_all_evidence_before_record_and_names_the_path() {
    let f=Fixture::new("mismatch",true);
    let mut writer=Writer {path:"/var/uitest/ui-testing/diagnosis/local".into(),files:vec![],error:None};
    let (v,o)=f.run(&mut writer,false);
    assert_eq!(v.failure_class(),Some(FailureClass::PixelMismatch));
    assert_eq!(writer.files.iter().map(|f| f.0.as_str()).collect::<Vec<_>>(),
        ["pixels.got.png","pixels.diff.png","tree-7.txt","input.log","screen.png","screen.record"]);
    assert_eq!(libimg::png::decode(&writer.files[0].1).unwrap().pixels,f.got);
    assert_eq!(writer.files[2].1,b"{\"commit\":41}");
    assert_eq!(writer.files[3].1,b"serial=1 disposition=processed\nserial=2 disposition=rejected\n");
    assert_eq!(libimg::png::decode(&writer.files[4].1).unwrap().pixels,f.want);
    assert_eq!(writer.files[5].1,vec![42;1024]);
    let line=&o.records()[0];
    assert!(line.ends_with("artifacts=complete path=/var/uitest/ui-testing/diagnosis/local"),"{line}");
    assert!(matches!(o,ScenarioOutcome::Fail {class:FailureClass::Infrastructure,..})); // mismatch cannot become PASS
    fs::remove_dir_all(&f.path).unwrap();
}
#[test]
fn pass_is_lazy_save_all_writes_and_io_or_cap_keeps_pixel_mismatch() {
    let f=Fixture::new("pass",false);
    let mut writer=Writer {path:"/var/uitest/ui-testing/diagnosis/local".into(),files:vec![],error:Some(SaveError::Io)};
    assert!(f.run(&mut writer,false).0.comparison.passed()); assert!(writer.files.is_empty());
    writer.error=None; let (_,saved)=f.run(&mut writer,true);
    assert_eq!(writer.files.len(),6); assert!(saved.records()[0].contains("artifacts=complete path="));
    fs::remove_dir_all(&f.path).unwrap();
    let f=Fixture::new("errors",true);
    for (error,token) in [(SaveError::Io,"none:io"),(SaveError::Cap,"none:cap")] {
        writer.error=Some(error); let(v,o)=f.run(&mut writer,false);
        assert_eq!(v.failure_class(),Some(FailureClass::PixelMismatch));
        assert!(o.records()[0].ends_with(&format!("artifacts={token}")),"{:?}",o.records());
        assert!(o.records()[0].contains("differing=1 first=1,0 got=#283263 want=#28323c"));
    }
    fs::remove_dir_all(&f.path).unwrap();
}
#[test]
fn record_path_is_strictly_bound_to_the_suite_case_and_complete_status() {
    let base=format!("SUITE ui-testing FRAME diagnosis/local pixels def=12345678 ref=formula source=surface commit=41 epoch=0 w=2 h=1 sha256={} result=mismatch compared=2 differing=1 first=1,0 got=#283263 want=#28323c artifacts=complete", "a".repeat(64));
    let accepts=|line:&str|result::Accounting::new("ui-testing","diagnosis/local",&["pixels"],false).unwrap().accept(line).is_ok();
    assert!(accepts(&format!("{base} path=/var/uitest/ui-testing/diagnosis/local")));
    for tail in ["/var/uitest/ui-testing/diagnosis/other","/var/uitest/ui-testing/diagnosis/../local","relative","/tmp/local"] {
        assert!(!accepts(&format!("{base} path={tail}")),"{tail}");
    }
    assert!(!accepts(&format!("{} path=/var/uitest/ui-testing/diagnosis/local",base.replace("artifacts=complete","artifacts=none:io"))));
}

#[test]
fn all_six_publications_finish_before_the_result_pipe_write() {
    use std::{rc::Rc,cell::Cell};
    struct Counting<'a> {writer:&'a mut Writer, count:Rc<Cell<usize>>}
    impl CheckWriter for Counting<'_> {
        fn path(&self)->&Path {self.writer.path()}
        fn write(&mut self,name:&str,bytes:&[u8])->Result<(),SaveError> {
            self.writer.write(name,bytes)?; self.count.set(self.count.get()+1); Ok(())
        }
    }
    let f=Fixture::new("before-record",true);
    let count=Rc::new(Cell::new(0)); let observed=count.clone();
    let sent=Rc::new(Cell::new(false)); let observed_sent=sent.clone();
    suite::AT_RECORD.with(|hook| *hook.borrow_mut()=Some(Box::new(move |line| {
        assert_eq!(observed.get(),6,"FRAME emitted before artifacts finished");
        assert!(line.contains("artifacts=complete path=")); observed_sent.set(true);
    })));
    let mut writer=Writer {path:"/var/uitest/ui-testing/diagnosis/local".into(),files:vec![],error:None};
    let mut writer=Counting {writer:&mut writer,count};
    f.run_mode(&mut writer,false,true);
    assert!(sent.get()); suite::AT_RECORD.with(|hook|hook.borrow_mut().take());
    fs::remove_dir_all(&f.path).unwrap();
}
#[test]
fn rename_failure_removes_temporary_output_and_keeps_existing_files() {
    let root=root("rename-error");
    let mut store=artifacts::Store::new(&root,"ui-testing","diagnosis/local").unwrap();
    store.write("kept",b"already synced").unwrap();
    let path=root.join("ui-testing/diagnosis/local");
    fs::create_dir(path.join("blocked")).unwrap();
    assert_eq!(CheckWriter::write(&mut store,"blocked",b"new data"),Err(SaveError::Io));
    assert!(!path.join("blocked.tmp").exists());
    assert_eq!(fs::read(path.join("kept")).unwrap(),b"already synced");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn local_writer_is_lazy_clears_stale_case_files_once_and_keeps_its_outputs() {
    let root=root("lazy-local");
    let path=root.join("ui-testing/diagnosis/local");
    let mut local=artifacts::Local::new(&root,"ui-testing","diagnosis/local");
    assert!(!path.exists());
    fs::create_dir_all(&path).unwrap();fs::write(path.join("stale"),b"old run").unwrap();
    local.write("first",b"new run").unwrap();
    assert!(!path.join("stale").exists());
    local.write("second",b"same run").unwrap();
    assert_eq!(fs::read(path.join("first")).unwrap(),b"new run");
    assert_eq!(fs::read(path.join("second")).unwrap(),b"same run");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn oversized_bundle_drops_screen_first_and_preserves_other_evidence() {
    let entries = vec![("got.png".to_string(), vec![1; 1024]),
        ("screen.png".to_string(), vec![2; 2 * 1024 * 1024])];
    let bundle = artifacts::bundle(&entries).unwrap();
    assert!(bundle.screen_dropped);
    assert!(bundle.bytes.len() <= 2 * 1024 * 1024);
    assert!(bundle.bytes.windows(7).any(|w| w == b"got.png"));
    assert!(!bundle.bytes.windows(10).any(|w| w == b"screen.png"));
}

#[test]
fn retention_freezes_synced_files_before_frame_and_labels_the_case() {
    let root = root("retention");
    let mut local = artifacts::Local::new(&root, "ui-testing", "diagnosis/local");
    local.write("got.png", b"first capture").unwrap();
    local.write("screen.png", &vec![2; 2 * 1024 * 1024]).unwrap();
    assert_eq!(local.prepare_retention(), Ok(Some(true)));
    let frozen = fs::read(local.path().join("retention.bundle")).unwrap();
    assert!(frozen.starts_with(b"UTART001\n"));
    assert!(!frozen.windows(10).any(|w| w == b"screen.png"));
    assert!(frozen.windows(13).any(|w| w == b"first capture"));
    assert_eq!(artifacts::label("diagnosis/local"), "uitest-19581b0bf17dadcb");
    // Collection can append or replace files without changing the advertised bundle.
    local.write("got.png", b"later collection").unwrap();
    assert_eq!(local.prepare_retention(), Ok(Some(true)));
    assert_eq!(fs::read(local.path().join("retention.bundle")).unwrap(), frozen);
    assert_eq!(fs::metadata(local.path().join("screen.png")).unwrap().len(), 2 * 1024 * 1024);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn bundle_refuses_oversized_non_screen_evidence_and_counts_framing() {
    let entries = vec![("got.png".to_string(), vec![1; 2 * 1024 * 1024])];
    assert!(artifacts::bundle(&entries).is_err());
    let bundle = artifacts::bundle(&[("got.png".into(), b"pixels".to_vec())]).unwrap();
    assert!(!bundle.screen_dropped);
    let header = format!("file=got.png bytes=6 sha256={}\n", frames::digest(b"pixels"));
    assert_eq!(bundle.bytes, [b"UTART001\n".as_slice(), header.as_bytes(), b"pixels"].concat());
}

#[test]
fn dropped_screen_is_reported_in_the_same_failure_frame() {
    struct Dropped(Writer);
    impl CheckWriter for Dropped {
        fn path(&self) -> &Path { self.0.path() }
        fn write(&mut self, name: &str, bytes: &[u8]) -> Result<(), SaveError> { self.0.write(name, bytes) }
        fn prepare_retention(&mut self) -> Result<Option<bool>, SaveError> { Ok(Some(true)) }
    }
    let f = Fixture::new("drop-record", true);
    let mut writer = Dropped(Writer {path:"/var/uitest/ui-testing/diagnosis/local".into(),files:vec![],error:None});
    let (v, outcome) = f.run(&mut writer, false);
    assert_eq!(v.failure_class(), Some(FailureClass::PixelMismatch));
    let line = &outcome.records()[0];
    assert!(line.ends_with("artifacts=partial:screen-dropped"), "{line}");
    result::Accounting::new("ui-testing", "diagnosis/local", &["pixels"], false).unwrap().accept(line).unwrap();
    fs::remove_dir_all(f.path).unwrap();
}

#[test]
fn seventeenth_bundle_stays_local_and_budget_leaves_collection_time() {
    let root = root("slots");
    for n in 0..16 {
        artifacts::reserve_upload(&root, &artifacts::label(&format!("diagnosis/case-{n}"))).unwrap();
    }
    assert_eq!(artifacts::reserve_upload(&root, &artifacts::label("diagnosis/seventeenth")).unwrap_err(), "slots");
    assert_eq!(artifacts::reserve_upload(&root, &artifacts::label("diagnosis/case-0")).unwrap_err(), "already-attempted");
    assert_eq!(artifacts::upload_budget_ms(10_000), 8250);
    assert_eq!(artifacts::upload_budget_ms(2000), 250);
    assert_eq!(artifacts::upload_budget_ms(1750), 0);
    assert_eq!(artifacts::upload_budget_ms(10), 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn repeated_full_screen_evidence_fits_the_existing_artifact_cap_losslessly() {
    let pixels = [45, 45, 45, 255].repeat(2560 * 1600);
    let encoded = artifacts::png(frame(&pixels, 2560, 1600)).unwrap();
    assert!(encoded.len() * 8 < artifacts::CAP_BYTES,
        "eight normal full-screen evidence pictures exceed the boot cap: {} bytes each", encoded.len());
    assert_eq!(libimg::png::decode(&encoded).unwrap().pixels, pixels);
}
