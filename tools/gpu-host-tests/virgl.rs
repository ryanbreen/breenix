// These modules expose the complete protocol catalogue; this test uses its draw path.
#[allow(dead_code)]
#[path = "../../kernel/src/gpu/virgl/wire.rs"]
pub(crate) mod wire;
#[allow(dead_code, unused_imports)]
#[path = "../../kernel/src/gpu/virgl/encode.rs"]
pub(crate) mod encode;

