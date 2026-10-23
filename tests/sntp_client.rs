#[path = "../kernel/src/time/slew.rs"]
mod slew;
#[path = "../userspace/programs/src/sntp.rs"]
mod sntp;

fn packet(t: i128) -> ([u8; 48], [u8; 8]) {
    let origin = sntp::timestamp(t);
    let mut p = [0u8; 48];
    p[0] = 0x24;
    p[1] = 2;
    p[16..24].copy_from_slice(&sntp::timestamp(t - sntp::NS));
    p[24..32].copy_from_slice(&origin);
    p[32..40].copy_from_slice(&sntp::timestamp(t + 20_000_000));
    p[40..48].copy_from_slice(&sntp::timestamp(t + 30_000_000));
    (p, origin)
}

#[test]
fn four_timestamps_remove_roundtrip_and_server_processing_across_era_rollover() {
    for secs in [1_790_000_000, 2_085_978_495, 2_085_978_496, 2_200_000_000] {
        let t = secs * sntp::NS + 990_000_000;
        let (p, origin) = packet(t);
        // 20 ms outbound + 10 ms processing + 20 ms inbound: expected offset is 0 ns.
        let reply = sntp::response(&p, origin, t, 50_000_000, true).unwrap();
        assert!(matches!(reply, sntp::Reply::Offset(v) if v.abs() <= 1));
    }
}

#[test]
fn rejects_bad_wire_and_unfit_clock_instead_of_accepting_transmit_alone() {
    let t = 1_790_000_000 * sntp::NS;
    let (good, origin) = packet(t);
    for field in [0, 1, 16, 24, 32, 40] {
        let mut p = good;
        if field == 0 {
            p[0] |= 0xc0;
        } else if field == 1 {
            p[1] = 16;
        } else {
            p[field..field + 8].fill(0);
        }
        assert!(
            sntp::response(&p, origin, t, 50_000_000, true).is_err(),
            "field {field}"
        );
    }
    let mut p = good;
    p[8..12].copy_from_slice(&(2u32 << 16).to_be_bytes());
    assert!(sntp::response(&p, origin, t, 50_000_000, true).is_err());
    assert!(sntp::response(&good, origin, t, 2_000_000_000, true).is_err());
    let (p, _) = packet(t + 1001 * sntp::NS);
    let mut p = p;
    p[24..32].copy_from_slice(&origin);
    assert!(sntp::response(&p, origin, t, 50_000_000, true).is_err());
    assert!(sntp::response(&p, origin, t, 50_000_000, false).is_ok());
}

#[test]
fn kiss_of_death_is_not_a_clock_sample() {
    let t = 1_790_000_000 * sntp::NS;
    let (mut p, origin) = packet(t);
    p[1] = 0;
    p[2] = 13;
    p[12..16].copy_from_slice(b"RATE");
    assert_eq!(
        sntp::response(&p, origin, t, 50_000_000, true),
        Ok(sntp::Reply::Rate(8192))
    );
    p[12..16].copy_from_slice(b"DENY");
    assert_eq!(
        sntp::response(&p, origin, t, 50_000_000, true),
        Ok(sntp::Reply::Denied)
    );
    p[24] ^= 1;
    assert!(sntp::response(&p, origin, t, 50_000_000, true).is_err());
}

#[test]
fn finite_signed_slew_is_continuous_monotonic_and_stops_at_requested_correction() {
    for delta in [-200_000_000, 200_000_000] {
        assert_eq!(slew::applied(delta, 0), 0);
        assert_eq!(
            slew::applied(delta, 1_000_000_000),
            delta.signum() * 500_000
        );
        assert_eq!(slew::applied(delta, 400_000_000_000), delta);
        assert_eq!(slew::applied(delta, u64::MAX), delta);
        let mut last = 0i128;
        for ms in 1..=401_000u64 {
            let mono = ms * 1_000_000;
            let real = i128::from(mono) + i128::from(slew::applied(delta, mono));
            assert!(real > last);
            last = real;
        }
    }
}
