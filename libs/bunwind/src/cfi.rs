#[cfg(test)]
#[test]
fn truncated_instructions_is_error_not_panic() {
    use crate::cfi::{Endian, ErrorKind, Program, Rules, Stream};

    // A complete single instruction in each entry. Test every cut after
    // its opcode, since a cut at an instruction boundary can be valid CFI.
    let instructions: &[&[u8]] = &[
        &[0x01, 4, 0, 0, 0],       // set_loc
        &[0x02, 1],                // advance_loc1
        &[0x03, 1, 0],             // advance_loc2
        &[0x04, 1, 0, 0, 0],       // advance_loc4
        &[0x0c, 1, 0x80, 1],       // def_cfa, multi-byte ULEB128 offset
        &[0x12, 1, 0xbf, 0x7f],    // def_cfa_sf, multi-byte SLEB128 offset
        &[0x81, 2],                // offset
        &[0x0f, 2, 0x31, 0x32],    // def_cfa_expression
        &[0x10, 1, 2, 0x31, 0x22], // expression
        &[0x16, 1, 2, 0x31, 0x22], // val_expression
    ];
    let program = Program {
        cie: &[0x0c, 1, 0],
        fde: &[],
        initial_location: 0,
        address_range: 64,
        code_alignment: 1,
        data_alignment: -8,
        address_size: 4,
        endian: Endian::Little,
    };
    for instruction in instructions {
        Program {
            fde: instruction,
            ..program
        }
        .rows(Rules::<2>::default(), &mut [], |_| {})
        .unwrap();
        for end in 1..instruction.len() {
            let error = Program {
                fde: &instruction[..end],
                ..program
            }
            .rows(Rules::<2>::default(), &mut [], |_| {})
            .unwrap_err();
            assert_eq!(
                (error.stream, error.offset, error.kind),
                (Stream::Fde, 0, ErrorKind::Truncated),
                "{:02x?}",
                &instruction[..end]
            );
        }
    }
    let error = Program {
        cie: &[0, 0x09, 1],
        ..program
    }
    .rows(Rules::<2>::default(), &mut [], |_| {})
    .unwrap_err();
    assert_eq!(
        (error.stream, error.offset, error.kind),
        (Stream::Cie, 1, ErrorKind::Truncated)
    );
}
