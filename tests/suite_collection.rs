#[path = "../libs/libbreenix/src/suite_result.rs"] pub mod result;
#[path = "../libs/libbreenix/src/suite_collection.rs"] pub mod collection;
use collection::{Handshake, Request, State, Trigger};

const RECORD:&str="SUITE test COLLECT cat/case trigger=fail screen=none:unavailable windows=0 remnants=0 artifacts=none:unavailable";
fn collecting()->Handshake<'static>{Handshake::new("test","cat/case",Request::Collect(Trigger::Fail))}

#[test]fn fragmented_record_and_done_precede_sweep_without_waiting_for_exit(){
    let mut h=collecting();
    assert_eq!(h.request().command(),"end collect fail\n");
    let wire=format!("{RECORD}\ndone\n");let mut records=Vec::new();
    for b in wire.bytes(){records.extend(h.feed(&[b]).unwrap());}
    assert_eq!(records,[RECORD]);
    assert_eq!(h.state(2999,3000,false),State::Done);
    // A reply queued before exit is drained before deciding supervisor-died.
    assert_eq!(h.state(2999,3000,true),State::Done);
    assert_eq!(h.state(3000,3000,false),State::Timeout);
}
#[test]fn silent_dead_and_missing_record_are_distinct(){
    let h=collecting();
    assert_eq!(h.state(1500,3000,false),State::Waiting);
    assert_eq!(h.state(2999,3000,false),State::Waiting);
    assert_eq!(h.state(3000,3000,false),State::Timeout);
    assert_eq!(h.state(1500,3000,true),State::Died);
    let mut h=collecting();h.feed(b"done\n").unwrap();
    assert_eq!(h.state(1500,3000,false),State::Timeout);
    let mut discard=Handshake::new("test","cat/case",Request::Discard);
    assert_eq!(discard.request().command(),"end discard\n");
    discard.feed(b"done\n").unwrap();
    assert_eq!(discard.state(100,3000,false),State::Done);
}
#[test]fn reject_forged_duplicate_wrong_trigger_or_non_collect_records(){
    for record in [RECORD.replace("cat/case","cat/other"), RECORD.replace("trigger=fail","trigger=crash"),
                   "SUITE test INPUT cat/case sent=0 processed=0 handled=0 rejected=0 lost=0".into(),"broken".into()]{
        assert!(collecting().feed(format!("{record}\n").as_bytes()).is_err());
    }
    let mut h=collecting();h.feed(format!("{RECORD}\n").as_bytes()).unwrap();
    assert!(h.feed(format!("{RECORD}\n").as_bytes()).is_err());
    assert!(Handshake::new("test","cat/case",Request::Discard).feed(format!("{RECORD}\n").as_bytes()).is_err());
    assert!(collecting().feed(&vec![b'x';4096]).is_err());
    assert!(collecting().feed(b"done\nextra\n").is_err());
}
#[test]fn commands_have_a_closed_trigger_vocabulary(){
    for (text,request) in [("end discard\n",Request::Discard),("end collect fail\n",Request::Collect(Trigger::Fail)),
        ("end collect crash\n",Request::Collect(Trigger::Crash)),("end collect timeout\n",Request::Collect(Trigger::Timeout)),
        ("end collect save-all\n",Request::Collect(Trigger::SaveAll))]{
        assert_eq!(Request::parse(text).unwrap(),request);assert_eq!(request.command(),text);
    }
    for text in ["end collect\n","end collect pass\n","end discard fail\n","end collect fail\nextra\n"]{assert!(Request::parse(text).is_err());}
}
