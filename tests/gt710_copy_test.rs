//! A0B5 packets must release completion through CE after the data transfer.
extern crate alloc;
#[path = "../kernel/src/gpu/gk208/copy.rs"] mod copy;
use copy::{encode, Rect};

fn methods(words: &[u32]) -> Vec<(u32, Vec<u32>)> {
    let mut result=Vec::new(); let mut at=0;
    while at<words.len() {
        let h=words[at]; let count=((h>>16)&0x1fff) as usize;
        assert_eq!(h>>29,1); assert_eq!((h>>13)&7,4);
        assert!(count>0); result.push(((h&0x1fff)*4,words[at+1..at+1+count].to_vec()));
        at+=count+1;
    }
    result
}
#[test]
fn rectangles_use_virtual_addresses_and_engine_ordered_epoch_releases() {
    let r=Rect{src:0x200ffd,dst:0x400fff,width:257,height:7,src_pitch:271,dst_pitch:283};
    let packets=methods(&encode(r,0x800000,0x12345678abcdef01,77).unwrap());
    assert_eq!(packets[0],(0,vec![0xa0b5]));
    assert!(packets.contains(&(0x400,vec![0,0x200ffd,0,0x400fff,271,283,257,7])));
    let launches:Vec<_>=packets.iter().filter(|(m,_)|*m==0x300).map(|(_,v)|v[0]).collect();
    assert_eq!(launches,vec![0x386,0x0c,0x0c,0x0c]);
    let sem:Vec<_>=packets.iter().filter(|(m,_)|*m==0x240).map(|(_,v)|v.clone()).collect();
    assert_eq!(sem,vec![vec![0,0x800004,0xabcdef01],vec![0,0x800008,0x12345678],vec![0,0x800000,77]]);
}
#[test]
fn encoder_rejects_overflow_bad_pitches_and_unstaged_overlap() {
    let good=Rect{src:0x200000,dst:0x400000,width:4097,height:2,src_pitch:4111,dst_pitch:8192};
    for r in [Rect{width:0,..good},Rect{height:0,..good},Rect{width:9000,..good},
              Rect{src:(1<<40)-4096,..good},Rect{dst:u64::MAX,..good},
              Rect{dst:good.src+1,..good}] {
        assert!(encode(r,0x800000,1,1).is_err());
    }
    assert!(encode(good,0x800001,1,1).is_err());
    assert!(encode(good,(1<<40)-8,1,1).is_err());
    assert!(encode(good,0x800000,0,1).is_err());
}

#[test]
fn suppressed_release_executes_the_same_data_launch_without_a_semaphore() {
    let r=Rect{src:0x400000,dst:0x500000,width:8192,height:1,src_pitch:8192,dst_pitch:8192};
    let p=methods(&copy::transfer(r).unwrap());
    assert_eq!(p.len(),3);
    assert_eq!(p[2],(0x300,vec![0x386]));
    assert!(!p.iter().any(|(m,_)|*m==0x240));
    assert_eq!(p,methods(&encode(r,0x108000,42,99).unwrap())[..3]);
}
