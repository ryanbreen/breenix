#[path = "../kernel/src/memory/stack_growth.rs"]
mod policy;

mod stack_growth {
    use super::policy::permits_stack_growth;
    use std::ops::Range;

    #[test]
    fn honors_rlimit_above_2mib() {
        const MIB: u64 = 1 << 20;
        const STACK_TOP: u64 = 128 * MIB;
        const STACK_BOTTOM: u64 = STACK_TOP - (64 << 10);
        const MAPPED_BYTES: u64 = 4 * MIB;

        let permits = |depth, stack_limit, address_space_limit, vmas: &[Range<u64>]| {
            permits_stack_growth(
                STACK_BOTTOM..STACK_TOP,
                STACK_TOP - depth,
                stack_limit,
                address_space_limit,
                || MAPPED_BYTES,
                vmas.iter().cloned(),
            )
        };

        assert!(
            permits(7 * MIB, 8 * MIB, u64::MAX, &[]),
            "an 8 MiB soft limit must permit growth 7 MiB below the stack top"
        );
        assert!(permits(8 * MIB, 8 * MIB, u64::MAX, &[]));
        assert!(!permits(9 * MIB, 8 * MIB, u64::MAX, &[]));
        assert!(!permits(3 * MIB, 2 * MIB, u64::MAX, &[]));
        assert!(permits(64 * MIB, u64::MAX, u64::MAX, &[]));
        assert!(!permits(64 * MIB + 4096, u64::MAX, u64::MAX, &[]));

        assert!(
            !permits(7 * MIB, 8 * MIB, 8 * MIB, &[]),
            "the new pages must fit under RLIMIT_AS"
        );
        let mapping = (STACK_TOP - 6 * MIB)..(STACK_TOP - 5 * MIB);
        assert!(
            !permits(7 * MIB, 8 * MIB, u64::MAX, &[mapping]),
            "growth must refuse a VMA anywhere between the fault page and the old bottom"
        );
    }
}
