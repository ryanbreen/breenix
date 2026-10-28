extern crate alloc;
#[path = "../kernel/src/gpu/completion.rs"] pub mod gpu_completion;
pub mod block { pub mod install { pub mod sha256 { pub use crate::sha256::*; } } }
#[path = "../libs/libbreenix/src/crypto/sha256.rs"] pub mod sha256;
#[path = "../kernel/src/gpu/gk208/core_record.rs"] pub mod core_record;
use core_record::{Completion, anchor, caps};
use gpu_completion::{Request, CeSample};
#[test]
fn encode_native_identity_and_control_without_virtual_success() {
    let r = Request { context: 0x123000, epoch: 17, caller: 9001, sequence: 6, resource: 0x456000, generation: 9 };
    let mut sample = CeSample { context: r.context, epoch: 16, sequence: 6, consumed: 6, healthy: true };
    let row = Completion { name: "copy", control: "stale-epoch", request: r, sample,
        kind: 0xa0b50300, answer: 0, elapsed_us: 123, outcome: "rejected" }.encode();
    assert_eq!(row, "GT710_CORE {\"schema\":1,\"type\":\"completion\",\"case\":\"copy\",\"control\":\"stale-epoch\",\"context\":1191936,\"epoch\":17,\"sequence\":6,\"caller\":9001,\"resource\":4546560,\"generation\":9,\"kind\":2696217344,\"answer\":0,\"observed_context\":1191936,\"observed_epoch\":16,\"observed_sequence\":6,\"consumed\":6,\"healthy\":true,\"elapsed_us\":123,\"outcome\":\"rejected\"}\n");
    assert!(r.complete(sample).is_none());
    sample.epoch = r.epoch;
    assert!(r.complete(sample).is_some());
    let row = Completion { name: "copy", control: "clean", request: r, sample,
        kind: 0xa0b50300, answer: gpu_abi::gk208::SUCCESS, elapsed_us: 124, outcome: "done" }.encode();
    assert!(row.contains(&format!("\"answer\":{}", gpu_abi::gk208::SUCCESS)));
    assert!(row.ends_with("\"outcome\":\"done\"}\n"));
}
#[test]
fn caps_and_anchor_cover_exact_wire_bytes() {
    let bytes: [u8; 512] = core::array::from_fn(|i| i as u8);
    let row = caps(&bytes);
    let expected: String = bytes.iter().map(|v| format!("{v:02x}")).collect();
    assert_eq!(row, format!("GT710_CORE {{\"schema\":1,\"type\":\"caps\",\"bytes\":512,\"hex\":\"{expected}\"}}\n"));
    assert_eq!(anchor(b"abc\n"), "GT710_CORE_BULK 4 edeaaff3f1774ad2888673770c6d64097e391bc362d7d6fb34982ddf0efd18cb");
    assert_ne!(anchor(row.as_bytes()), anchor(row.trim_end().as_bytes()));
}
