#[path = "../libs/libbreenix/src/evidence_reference.rs"]
mod subject;

#[test]
fn only_completed_earlier_identical_parts_can_be_referenced() {
    let mut cache=subject::Cache::new();
    let hash=[0xab;32];
    assert!(cache.reference(1,2<<20,hash).is_none());
    cache.remember(0,2<<20,hash).unwrap();
    let request=cache.reference(1,2<<20,hash).expect("duplicate part retransmitted");
    assert_eq!(request,format!("PICTURE_REF v=1 source=0 bytes=2097152 sha256={}\n","ab".repeat(32)));
    assert!(request.len() < 2<<20);
    assert!(cache.reference(0,2<<20,hash).is_none());
    assert!(cache.reference(1,1024,hash).is_none());
    assert!(cache.reference(1,2<<20,[1;32]).is_none());
    assert!(cache.remember(1,0,hash).is_err());
    assert!(cache.remember(1,(2<<20)+1,hash).is_err());
    cache.remember(1,2<<20,hash).unwrap();
    assert!(cache.remember(1,2<<20,hash).is_err());
}
