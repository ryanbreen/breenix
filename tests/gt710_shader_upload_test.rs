#[path = "../kernel/src/gpu/gk208/gr/shaders.rs"]
mod shaders;
#[path = "../libs/libbreenix/src/crypto/sha256.rs"]
mod sha256;

#[derive(Default)]
struct Model { vram: Vec<u8>, records: Vec<(shaders::UploadCase, Vec<u8>)> }
impl shaders::UploadIo for Model {
    type Error = &'static str;
    fn poison(&mut self, byte: u8) -> Result<(), Self::Error> { self.vram = vec![byte; shaders::SEGMENT_SIZE]; Ok(()) }
    fn upload(&mut self, bytes: &[u8]) -> Result<(), Self::Error> { self.vram.copy_from_slice(bytes); Ok(()) }
    fn readback(&mut self, case: shaders::UploadCase, expected: &[u8]) -> Result<(), Self::Error> {
        assert_eq!(self.vram, expected);
        self.records.push((case, self.vram.clone())); Ok(())
    }
}
#[test]
fn upload_controls_and_recovery_have_expected_digests() {
    let mut model = Model::default();
    shaders::exercise_uploads(&mut model).unwrap();
    let cases = [shaders::UploadCase::Withheld, shaders::UploadCase::AfterWithheld, shaders::UploadCase::Flipped, shaders::UploadCase::AfterFlipped];
    let hashes = ["46721af15bab721c92d206e0e7756231c93639ce5002c110d8dfb35490769d90", "c513380002cf61a35211c22e993bfd8a7bea6c2d831cee6581bec427f07bd4c0", "23248e6e3e4923cc014dee19e44c562bf50b329c8c3378a69fe0239be7f92b4c", "c513380002cf61a35211c22e993bfd8a7bea6c2d831cee6581bec427f07bd4c0"];
    assert_eq!(model.records.len(), cases.len());
    for ((case, bytes), (expected_case, expected_hash)) in model.records.iter().zip(cases.into_iter().zip(hashes)) {
        assert_eq!(*case, expected_case);
        assert_eq!(bytes.len(), 640);
        let actual: String = sha256::sha256(bytes).iter().map(|v| format!("{v:02x}")).collect();
        assert_eq!(actual, expected_hash);
    }
}
