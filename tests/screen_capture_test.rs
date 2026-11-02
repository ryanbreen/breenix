use screen_capture::{decode, encode, CaptureLog, Record, HEADER};

fn record(pixels: &[u8]) -> Record<'_> {
    Record { suite: "gpu", case: "draw/one", checkpoint: "end", width: (pixels.len()/4) as u32,
        height: 1, digest: libpixcheck::sha256(pixels), pixels }
}

#[test]
fn scr1_round_trips_and_selects_the_smaller_payload() {
    for (pixels, encoding) in [(vec![1,2,3,255].repeat(128), 1u32),
        ((0..32u8).flat_map(|n| [n,0,0,255]).collect(), 0), (vec![1,2,3,255].repeat(2), 0)] {
        let input = record(&pixels);
        let bytes = encode(&input).unwrap();
        assert_eq!(&bytes[..4], b"SCR1");
        assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), encoding);
        let (output, used) = decode(&bytes).unwrap();
        assert_eq!(used, bytes.len());
        assert_eq!(output.pixels, pixels);
        assert_eq!((output.suite.as_str(), output.case.as_str(), output.checkpoint.as_str()),
            ("gpu", "draw/one", "end"));
        assert_eq!((output.width,output.height,output.digest), (input.width,input.height,input.digest));
        assert_eq!(bytes.len() % 512, 0);
    }
}

#[test]
fn scr1_refuses_wrong_digests_lengths_and_damaged_payloads() {
    let pixels = vec![1,2,3,255].repeat(128);
    let mut input = record(&pixels);
    input.digest[0] ^= 1;
    assert!(encode(&input).is_err());
    input = record(&pixels); input.width += 1;
    assert!(encode(&input).is_err());
    let good = encode(&record(&pixels)).unwrap();
    let payload = HEADER + "gpu".len() + "draw/one".len() + "end".len();
    for index in [0, 8, 16, 32, payload, payload+4] {
        let mut bad = good.clone(); bad[index] ^= 1;
        assert!(decode(&bad).is_err(), "accepted changed byte {index}");
    }
    assert!(decode(&good[..good.len()-1]).is_err());
    let mut bad = good.clone(); bad[payload..payload+4].fill(0);
    assert!(decode(&bad).is_err());
    let mut bad = good.clone(); *bad.last_mut().unwrap() = 1;
    assert!(decode(&bad).is_err());
}

#[test]
fn capture_log_writes_only_first_digest_and_never_commits_failed_io() {
    let mut log = CaptureLog::new();
    let mut disk = vec![0; 4096];
    let pixels = vec![1,2,3,255].repeat(128);
    let bytes = encode(&record(&pixels)).unwrap();
    let mut writes = 0;
    let mut write = |offset: u64, data: &[u8]| {
        writes += 1;
        disk[offset as usize..offset as usize+data.len()].copy_from_slice(data); Ok(())
    };
    assert_eq!(log.append(&bytes,4096,&mut write), Ok(true));
    let mut alias = record(&pixels); alias.suite = "breengel"; alias.case = "draw/two";
    assert_eq!(log.append(&encode(&alias).unwrap(),4096,&mut write), Ok(false));
    assert_eq!(writes,1);
    assert_eq!(decode(&disk[512..]).unwrap().0.digest, alias.digest);
    let next = encode(&record(&[4,5,6,255])).unwrap();
    assert!(log.append(&next,4096,|_,_| Err("io")).is_err());
    assert!(log.append(&next,4096,|_,_| panic!("failed disk must stay poisoned")).is_err());
    let mut small = CaptureLog::new();
    assert!(small.append(&bytes,512,|_,_| panic!("out of bounds write")).is_err());
}

thread_local! {
    static ALLOCATION_LIMIT: std::cell::Cell<usize> = const { std::cell::Cell::new(usize::MAX) };
}
struct LimitedAllocator;
unsafe impl std::alloc::GlobalAlloc for LimitedAllocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        if layout.size() > ALLOCATION_LIMIT.try_with(|n| n.get()).unwrap_or(usize::MAX) {
            return std::ptr::null_mut();
        }
        unsafe { std::alloc::System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        unsafe { std::alloc::System.dealloc(ptr,layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, size: usize) -> *mut u8 {
        if size > ALLOCATION_LIMIT.try_with(|n| n.get()).unwrap_or(usize::MAX) {
            return std::ptr::null_mut();
        }
        unsafe { std::alloc::System.realloc(ptr,layout,size) }
    }
}
#[global_allocator]
static ALLOCATOR: LimitedAllocator = LimitedAllocator;

#[test]
fn capture_writer_needs_no_second_frame_sized_allocation() {
    let pixels: Vec<u8> = (0..2560*1600u32).flat_map(|i| [i as u8,(i>>8) as u8,(i>>16) as u8,255]).collect();
    let bytes = encode(&Record { width:2560,height:1600,..record(&pixels) }).unwrap();
    assert_eq!(bytes[8],0,"the full-screen non-repeating frame uses raw encoding");
    let mut log = CaptureLog::new();
    ALLOCATION_LIMIT.with(|n| n.set(4096));
    let result = log.append(&bytes,32*1024*1024,|_,_| Ok(()));
    ALLOCATION_LIMIT.with(|n| n.set(usize::MAX));
    assert_eq!(result,Ok(true),"valid raw capture must not allocate a decoded frame in the writer");
}

#[test]
fn streamed_capture_is_bounded_and_commits_header_after_verified_payload() {
    let pixels: Vec<u8> = (0..2560*1600u32).flat_map(|i| [i as u8,(i>>8) as u8,(i>>16) as u8,255]).collect();
    let bytes=encode(&Record { width:2560,height:1600,..record(&pixels) }).unwrap();
    let mut disk=vec![0;bytes.len()+512];
    let mut offsets=Vec::with_capacity(bytes.len()/512);
    let mut flushed=false;
    let mut log=CaptureLog::new();
    ALLOCATION_LIMIT.with(|n| n.set(4096));
    let result=log.append_stream(bytes.len(),disk.len() as u64,|at,buf| {
        assert!(buf.len()<=512);buf.copy_from_slice(&bytes[at..at+buf.len()]);Ok(())
    },|at,buf| { offsets.push(at);disk[at as usize..at as usize+buf.len()].copy_from_slice(buf);Ok(()) },
        || {flushed=true;Ok(())});
    ALLOCATION_LIMIT.with(|n| n.set(usize::MAX));
    assert_eq!(result,Ok(true)); assert!(flushed);
    assert_eq!(offsets[0],1024);assert_eq!(offsets.last(),Some(&512));
    assert_eq!(&disk[512..],&bytes);
    assert_eq!(log.append_stream(bytes.len(),disk.len() as u64,|at,buf| {
        buf.copy_from_slice(&bytes[at..at+buf.len()]);Ok(())
    },|_,_|panic!("duplicate write"),||panic!("duplicate flush")),Ok(false));
    let mut bad=encode(&record(&[1,2,3,255])).unwrap();bad[32]^=1;
    let mut log=CaptureLog::new();
    assert!(log.append_stream(bad.len(),4096,|at,buf| {
        buf.copy_from_slice(&bad[at..at+buf.len()]);Ok(())
    },|_,_|panic!("invalid digest committed"),||panic!("invalid digest flushed")).is_err());
    let mut log=CaptureLog::new();
    assert!(log.append_stream(bytes.len(),disk.len() as u64,|at,buf| {
        buf.copy_from_slice(&bytes[at..at+buf.len()]);Ok(())
    },|_,_|Err("io"),||Ok(())).is_err());
    assert!(log.append_stream(bytes.len(),disk.len() as u64,|_,_|panic!("poisoned read"),
        |_,_|panic!("poisoned write"),||Ok(())).is_err());
}
