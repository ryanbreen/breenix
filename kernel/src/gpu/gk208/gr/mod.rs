//! Static PGRAPH inputs. Device initialization and firmware startup follow separately.
pub mod firmware;
pub mod tables;
pub mod topology;

/// Keep the static images in the linked kernel until the firmware loader takes
/// over this suite-route stub. This checks source bytes without touching GR.
pub fn probe_static_inputs() -> Result<(), &'static str> {
    for (words, expected) in [
        (&firmware::FECS_CODE_BIN[..], firmware::FECS_CODE_BIN_SHA256),
        (&firmware::FECS_DATA_BIN[..], firmware::FECS_DATA_BIN_SHA256),
        (
            &firmware::GPCCS_CODE_BIN[..],
            firmware::GPCCS_CODE_BIN_SHA256,
        ),
        (
            &firmware::GPCCS_DATA_BIN[..],
            firmware::GPCCS_DATA_BIN_SHA256,
        ),
    ] {
        let mut hash = crate::block::install::sha256::Sha256::new();
        for word in words {
            hash.update(&word.to_le_bytes());
        }
        if super::hex(&hash.finalize()) != expected {
            return Err("GR static firmware integrity mismatch");
        }
    }
    Ok(())
}
