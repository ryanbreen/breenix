// Run the client submit path's scratch preparation 100 times on one target.
struct ScratchTarget {
    format: u32,
    width: u32,
    height: u32,
    native_blend_scratch: Vec<Arc<NativeScratch>>,
}
struct ScratchCore { resources: BTreeMap<u32, ScratchTarget> }
thread_local! { static SCRATCH_CORE: RefCell<ScratchCore> = RefCell::new(ScratchCore {
    resources: BTreeMap::new(),
}); }
fn with_core<R>(f: impl FnOnce(&mut ScratchCore) -> R) -> Option<R> {
    Some(SCRATCH_CORE.with(|c| f(&mut c.borrow_mut())))
}

#[test]
fn hundred_coverage_calls_allocate_one_snapshot_and_preserve_destination_bytes() {
    fresh();
    let sizes = [[48, 17], [0, 0]];
    with_core(|c| { c.resources.insert(7, ScratchTarget {
        format: 0, width: 48, height: 17, native_blend_scratch: Vec::new(),
    }); });
    with_core(|c| {
        let r=&c.resources[&7];
        assert_eq!((r.format,r.width,r.height,r.native_blend_scratch.len()),(0,48,17,0));
    });
    let mut actual: Vec<u8> = (0..48*17*4).map(|i| (i*37) as u8).collect();
    let mut reference = actual.clone();
    let mut first = None;
    let mut ids = std::collections::BTreeSet::new();
    for call in 0..100 {
        let scratch = (|| -> Result<NativeScratch,i64> { Ok(PREPARE) })().unwrap();
        let t = scratch.targets[0].unwrap(); ids.insert(t.id);
        first.get_or_insert(t.id);
        assert_eq!((t.width,t.height,t.format),(48,17,0));
        // Model the CE destination copy at the production layout's byte offsets.
        // The 48x17 destination, including pixels outside this draw, is refreshed.
        ce::with_resources(|r,_,io| {
            let l=r.layout(t.id).unwrap();let va=r.address(t.id).unwrap();
            let physical=(io.words[&r.pte_address(va).unwrap()]&0xfffffff0)<<8;
            for y in 0..17 { for x in (0..48).step_by(2) {
                let at=(y*48+x) as usize*4;
                io.words.insert(physical+l.offset(x,y,0).unwrap(),
                    u64::from_le_bytes(actual[at..at+8].try_into().unwrap()));
            }}
            let per_call = reference.clone();
            for y in 0..17 { for x in 0..48 {
                let at=(y*48+x) as usize*4;
                let pair=io.words[&(physical+l.offset(x&!1,y,0).unwrap())].to_le_bytes();
                let offset=(x as usize%2)*4;
                assert_eq!(&pair[offset..offset+4],&per_call[at..at+4]);
                // Vary mask, colour and write rectangle so stale copies are visible.
                if (x+y+call)%3==0 {
                    let a=((x*13+y*7+call)%256) as u32;
                    for c in 0..4 {
                        let src=((call*11+c as u32*29)%256)*a/255;
                        actual[at+c]=(src+(pair[offset+c] as u32*(255-a)+127)/255).min(255) as u8;
                        reference[at+c]=(src+(per_call[at+c] as u32*(255-a)+127)/255).min(255) as u8;
                    }
                }
            }}
            Ok(())
        }).unwrap();
        assert_eq!(actual,reference,"coverage output differs at call {call}");
        drop(scratch);collect();
    }
    assert_eq!(ids.len(),1,"100 calls must reuse one scratch allocation");
    assert_eq!(CHARGED.load(Ordering::SeqCst),8192);
    assert_eq!(RELEASES.load(Ordering::SeqCst),0,"target owns scratch between calls");
    let held=ce::with_resources(|r,_,_|r.hold(first.unwrap())).unwrap();
    with_core(|c| { c.resources.remove(&7); }); collect();
    assert_eq!(CHARGED.load(Ordering::SeqCst),8192,"queued device work retains storage");
    drop(held);collect();
    assert_eq!(CHARGED.load(Ordering::SeqCst),0);
    assert_eq!(RELEASES.load(Ordering::SeqCst),1);
}
