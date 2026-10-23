//! libbreenix CSPRNG checks requiring Breenix's getrandom syscall.

use libbreenix::crypto::rand::Csprng;
use breenix_suite::suite::{case, category, check, suite, CaseResult, Suite};

static SUITE: Suite = suite("crypto", "Cryptographic randomness", &[
    category("csprng", "CSPRNG", &[
        case("fill", "fill produces nonzero output", fill),
        case("across-blocks", "fill initializes every block across a 200-byte request", across_blocks),
        case("independent", "Independent CSPRNG instances produce different output", independent),
    ]),
]);

fn fill() -> CaseResult {
    let mut rng = Csprng::new();
    let mut bytes = [0u8; 64];
    rng.fill(&mut bytes);
    check(bytes.iter().any(|&b| b != 0), "fill produced all-zero output")
}

fn across_blocks() -> CaseResult {
    let mut rng = Csprng::new();
    let mut bytes = [0u8; 200];
    rng.fill(&mut bytes);
    // Check the final partial block as well as each complete ChaCha20 block.
    check(bytes.chunks(64).all(|block| block.iter().any(|&b| b != 0)),
        "fill left a block or the final partial block all zero")
}

fn independent() -> CaseResult {
    let mut first = Csprng::new();
    let mut second = Csprng::new();
    check(first.random_u64() != second.random_u64(),
        "independent CSPRNG instances produced identical output")
}

fn main() {
    SUITE.run()
}
