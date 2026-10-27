//! Common request identity and native two-poison publication (M6.3).
#[path = "../kernel/src/gpu/completion.rs"] pub mod gpu_completion;
#[path = "../kernel/src/gpu/observation.rs"] pub mod gpu_observation;
use gpu_completion::{CeSample, Request};
use gpu_observation::{native_record, ResourceVersion};

fn request(sequence: u32, caller: u64) -> Request {
    Request { context: 0x123000, epoch: 17, caller, sequence, resource: 0x456000, generation: 9 }
}
fn sample(r: Request) -> CeSample {
    CeSample { context: r.context, epoch: r.epoch, sequence: r.sequence, consumed: r.sequence, healthy: true }
}
#[test]
fn native_release_requires_context_epoch_sequence_and_progress() {
    let r = request(3, 9001);
    assert_eq!(r.complete(sample(r)).unwrap().request(), r);
    for s in [CeSample { context: 0x124000, ..sample(r) },
              CeSample { epoch: 16, ..sample(r) }, CeSample { sequence: 2, ..sample(r) },
              CeSample { consumed: 2, ..sample(r) }, CeSample { healthy: false, ..sample(r) }] {
        assert!(r.complete(s).is_none());
    }
    for bad in [Request { caller: 0, ..r }, Request { sequence: 0, ..r },
                Request { epoch: 0, ..r }, Request { resource: 0, ..r }] {
        assert!(bad.complete(sample(bad)).is_none());
    }
    let other = Request { context: r.context + 4096, ..r };
    assert!(other.complete(sample(r)).is_none());
    // Caller admission order is independent of the native queue timeline.
    assert!(request(4, 2).complete(sample(request(4, 2))).is_some());
    assert!(gpu_completion::matches_echo(Some(9001), Some(3), 3, true, 9001));
    assert!(!gpu_completion::matches_echo(Some(9001), Some(3), 9001, true, 9001));
    assert!(!gpu_completion::matches_echo(Some(9001), Some(3), 3, false, 9001));
}
#[test]
fn publication_binds_both_complete_transfers_to_one_resource_generation() {
    let w = request(1, 9001); let a = request(2, 23); let b = request(3, 24);
    let done = |r: Request| r.complete(sample(r)).unwrap();
    let version = ResourceVersion { context: w.context, epoch: w.epoch, resource: w.resource, generation: w.generation };
    let pixels = [0x55, 0xaa, 0, 255, 42];
    let record = native_record(version, done(w), [done(a), done(b)], &pixels, &pixels).unwrap();
    assert_eq!(record.version, version); assert_eq!(record.bytes, 5); assert_eq!(record.unwritten, 0);
    assert_eq!(record.writer.request().caller, 9001);
    assert_eq!(record.transfers[0].request().caller, 23);
    for wrong in [ResourceVersion { resource: version.resource + 4096, ..version },
                  ResourceVersion { generation: 10, ..version }, ResourceVersion { epoch: 18, ..version },
                  ResourceVersion { context: version.context + 4096, ..version }] {
        assert!(native_record(wrong, done(w), [done(a), done(b)], &pixels, &pixels).is_err());
    }
    let mut corrupt = pixels; corrupt[4] ^= 1;
    assert!(native_record(version, done(w), [done(a), done(b)], &pixels, &corrupt).is_err());
    assert!(native_record(version, done(w), [done(a), done(b)], &pixels, &pixels[..4]).is_err());
    assert!(native_record(version, done(w), [done(a), done(a)], &pixels, &pixels).is_err());
    assert!(native_record(version, done(w), [done(b), done(a)], &pixels, &pixels).is_err());
    assert!(native_record(version, done(w), [done(w), done(b)], &pixels, &pixels).is_err());
    let stale = Request { epoch: 16, ..b };
    assert!(native_record(version, done(w), [done(a), done(stale)], &pixels, &pixels).is_err());
}
