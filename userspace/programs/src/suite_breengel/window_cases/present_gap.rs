//! Join successful scene receipts and native edges in the rate case's interval.

use std::{collections::BTreeMap, fmt::Write};

// Stage and column definitions are in kernel/src/window/pipeline.rs.
type Row = [u64; 5];

#[derive(Clone, Copy)]
struct Frame {
    serial: u64,
    commit: u64,
    target: u64,
    presented: u64,
    receipt: u64,
}

pub(super) fn lines(
    rows: &[Row],
    window: u64,
    start: u64,
    end: u64,
    composed: u64,
) -> Option<[String; 3]> {
    let receipts: BTreeMap<_, _> = rows.iter()
        .filter(|r| r[0] == 13 && r[1] == window)
        .map(|r| (r[3], (r[2], r[4])))
        .collect();
    let mut frames: Vec<_> = rows.iter()
        .filter(|r| r[0] == 12 && (start..=end).contains(&r[4]))
        .filter_map(|r| {
            let &(commit, receipt) = receipts.get(&r[1])?;
            Some(Frame { serial: r[1], commit, target: r[2], presented: r[4], receipt })
        })
        .collect();
    frames.sort_unstable_by_key(|f| f.serial);
    // Stage 12 alone also includes frames whose scene finish failed. Stage 13
    // joins successful ones. The measured count detects an overwritten prefix
    // or missing entry before we present a surviving pair as the longest gap.
    if frames.first()?.presented != start || frames.last()?.presented != end
        || frames.len() as u64 != composed.checked_add(1)?
        || frames.windows(2).any(|f| f[0].serial == f[1].serial || f[0].presented >= f[1].presented)
    {
        return None;
    }
    let (index, pair) = frames.windows(2).enumerate()
        .max_by_key(|(_, f)| f[1].presented - f[0].presented)?;
    let (before, after) = (pair[0].presented, pair[1].presented);
    let gap = after - before;
    let previous = index.checked_sub(1).map(|i| frames[i]);
    Some([
        format!("GT710_PRESENT_TIMELINE gap_ns={gap} before_ns={before} after_ns={after} start_ns={start} end_ns={end} composites={composed} window={window}"),
        describe(rows, window, pair[0], previous),
        describe(rows, window, pair[1], Some(pair[0])),
    ])
}

fn describe(rows: &[Row], window: u64, frame: Frame, previous: Option<Frame>) -> String {
    let Frame { serial, commit, target, presented, receipt } = frame;
    let by_frame = |stage| rows.iter().find(|r| r[0] == stage && r[1] == serial);
    let by_edge = |stage| rows.iter().find(|r| r[0] == stage && r[1] == target && r[2] == presented);
    let front_ready = rows.iter().find(|r| r[0] == 14 && r[1] == window && r[2] == commit);
    let submit = by_frame(47);
    let native = by_edge(48);
    let prediction = by_edge(50);
    let raster = prediction.and_then(|p| rows.iter().find(|r| r[0] == 15 && r[1] == target && r[4] == p[4]));
    let first_ready = previous.and_then(|p| rows.iter()
        .filter(|r| r[0] == 14 && r[1] == window && p.commit < r[2] && r[2] <= commit)
        .map(|r| r[4]).min());
    let scene_read = previous.zip(submit).and_then(|(p, s)| rows.iter()
        .filter(|r| r[0] == 49 && (p.receipt..=s[2]).contains(&r[4]))
        .map(|r| r[4]).max());
    let mut line = format!("GT710_PRESENT_TIMELINE gap_frame serial={serial} commit={commit} target={target}");
    for (name, stamp) in [
        ("first_ready_ns", first_ready),
        ("front_ready_ns", front_ready.map(|r| r[4])),
        ("scene_read_ns", scene_read),
        ("compose_begin_ns", submit.map(|r| r[2])),
        ("scene_admit_ns", rows.iter().find(|r| r[0] == 9 && r[1] == window && r[3] == serial).map(|r| r[4])),
        ("submit_begin_ns", submit.map(|r| r[3])),
        ("submit_return_ns", submit.map(|r| r[4])),
        ("render_ready_ns", by_frame(10).map(|r| r[4])),
        ("native_accept_ns", prediction.map(|r| r[4])),
        ("native_accept_vert", raster.map(|r| r[2])),
        ("native_accept_pixel", raster.map(|r| r[3])),
        ("predicted_latch_ns", prediction.map(|r| r[3])),
        ("presented_ns", Some(presented)),
        ("native_complete_ns", native.map(|r| r[3])),
        ("native_observed_ns", native.map(|r| r[4])),
        ("present_return_ns", by_frame(11).map(|r| r[4])),
        ("receipt_ns", Some(receipt)),
    ] {
        match stamp {
            Some(stamp) => write!(line, " {name}={stamp}"),
            None => write!(line, " {name}=-"),
        }.expect("write to a String");
    }
    line
}
