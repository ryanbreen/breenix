/*******************************************************************************
    Copyright (c) 2020, NVIDIA CORPORATION. All rights reserved.

    Permission is hereby granted, free of charge, to any person obtaining a
    copy of this software and associated documentation files (the "Software"),
    to deal in the Software without restriction, including without limitation
    the rights to use, copy, modify, merge, publish, distribute, sublicense,
    and/or sell copies of the Software, and to permit persons to whom the
    Software is furnished to do so, subject to the following conditions:

    The above copyright notice and this permission notice shall be included in
    all copies or substantial portions of the Software.

    THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
    IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
    FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
    THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
    LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
    FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
    DEALINGS IN THE SOFTWARE.

*******************************************************************************/
// Oracle: MIT NVIDIA open-gpu-doc 9fdf5c40 classes/3d/cla197.h
// lines 34-36, 3274-3315, 3342-3350 and classes/host/cla06f.h
// lines 69-72, 135-150, 182-187. Expected words below are hand derived.
extern crate alloc;
#[path = "../kernel/src/gpu/gk208/gr/channel.rs"]
mod channel;
use channel::{Identity, Observation, Poll, Release, Report, Stale, Withheld};
use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn a197_bind_and_one_word_release_match_the_published_bit_fields() {
    // A06F DMA_INCR: opcode 1 << 29, count << 16, subchannel 0 << 13,
    // method byte offset / 4. A197 SET_OBJECT at 0 has class ID 0xa197.
    assert_eq!(channel::SUBCHANNEL, 0);
    assert_eq!(channel::bind(), [0x20010000, 0x0000a197]);
    // Four incrementing methods from 0x1b00 / 4 = 0x6c0; address A is
    // bits 39:32, B is bits 31:0, C is the payload, D is the operation.
    // D = RELEASE 0 | writes-complete (1<<4) | PIPELINE_LOCATION_ALL (15<<12) |
    // ONE_WORD (1<<28); FLUSH_DISABLE=0, report/reduction/awaken=0.
    assert_eq!(
        channel::report(0x12_34567000, 0x89abcdef).unwrap(),
        [0x200406c0, 0x12, 0x34567000, 0x89abcdef, 0x1000f010]
    );
    assert!(channel::report(1 << 40, 1).is_err());
    assert!(channel::report(0x600001, 1).is_err());
    let command = channel::commands(0x600000, 0x1122334455667788, 0x12345678, true, 7).unwrap();
    assert_eq!(
        command,
        [
            0x20010000, 0xa197, 0x200406c0, 0, 0x600004, 0x55667788, 0x1000f010, 0x200406c0, 0,
            0x600008, 0x11223344, 0x1000f010, 0x200406c0, 0, 0x600000, 0x12345678, 0x1000f010,
            0x20010014, 7
        ]
    ); // A06F SET_REFERENCE at 0x50 / 4.
       // Withhold D itself, while publishing A..C and SET_REFERENCE. The
       // stream still executes; neither epoch nor payload may be released.
    assert_eq!(
        channel::commands(0x600000, 42, 0x12345678, false, 8).unwrap(),
        [0x20010000, 0xa197, 0x200306c0, 0, 0x600000, 0x12345678, 0x20010014, 8]
    );
}

#[test]
fn receipt_domains_do_not_collide_when_epoch_bits_are_set() {
    for epoch in [1, 0x6000_0000, 0xffff_ffff, 0xffff_ffff_ffff_ffff] {
        let channel = channel::receipt_nonce(epoch, false);
        let binding = channel::receipt_nonce(epoch, true);
        assert_ne!(channel, binding);
        assert_eq!(channel & 0x6000_0000, 0x4000_0000);
        assert_eq!(binding & 0x6000_0000, 0x2000_0000);
    }
}

struct Mock {
    value: u32,
    epoch: u64,
    consumed: bool,
    now: u64,
}
impl Poll for Mock {
    fn snapshot(&mut self) -> Result<(u32, u64, bool), &'static str> {
        Ok((self.value, self.epoch, self.consumed))
    }
    fn now_us(&mut self) -> u64 {
        self.now += 10;
        self.now
    }
    fn relax(&mut self) {}
}
#[test]
fn withheld_and_executed_old_epoch_waits_expire_then_clean_releases_complete() {
    for (value, epoch) in [(85, 0), (0x12345678, 41)] {
        let mut io = Mock {
            value,
            epoch,
            consumed: true,
            now: 0,
        };
        let missing = channel::wait(&mut io, 42, 0x12345678, 10000).unwrap();
        assert!(!missing.accepted);
        assert!(missing.consumed);
        assert_eq!((missing.value, missing.epoch), (value, epoch));
        assert!(missing.elapsed_us >= 10000);
        io.value = 0x12345678;
        io.epoch = 42;
        let clean = channel::wait(&mut io, 42, 0x12345678, 2000000).unwrap();
        assert!(clean.accepted);
        assert!(clean.consumed);
        assert!(clean.elapsed_us < 10000);
    }
    let mut io = Mock {
        value: 0x12345678,
        epoch: 42,
        consumed: false,
        now: 0,
    };
    assert!(
        !channel::wait(&mut io, 42, 0x12345678, 10000)
            .unwrap()
            .accepted
    );
}

fn fixture() -> Report {
    let mut top = [0; 64];
    top[..2].copy_from_slice(&[0x80000036, 3]);
    let release = |poison| Release {
        target: 0x600000,
        poison,
        nonce: 0x12345678,
        observed: 0x12345678,
        epoch: 42,
        elapsed_us: 100,
    };
    Report {
        identity: Identity {
            id: 0,
            epoch: 42,
            instance: 0x10000000,
            top,
            engine: 0,
            runlist: 0,
            bind: 0x80010000,
            state: 1,
            current_context: 0x80010000,
        },
        releases: [release(85), release(170)],
        withheld: Withheld {
            poison: 85,
            observation: Observation {
                value: 85,
                epoch: 0,
                consumed: true,
                elapsed_us: 10000,
                accepted: false,
            },
        },
        stale: Stale {
            nonce: 0x12345678,
            observation: Observation {
                value: 0x12345678,
                epoch: 41,
                consumed: true,
                elapsed_us: 10000,
                accepted: false,
            },
        },
        faults: [[0x400100, 0], [0x400108, 0], [0x259c, 0]],
        runlist_count: 0,
        binding: 0,
    }
}
#[test]
fn emitted_gt710_gr_fixture_passes_the_u1_comparator() {
    let source = std::path::Path::new(file!()).canonicalize().unwrap();
    let root = source.parent().unwrap().parent().unwrap();
    let prefix = Command::new("python3")
        .current_dir(root)
        .args([
            "-I",
            "-c",
            r#"
import json,runpy
f=runpy.run_path('tests/gt710_graphics_report_test.py')['fixture']()
for k in ('channel','releases','withheld_release','stale_release','faults','teardown'): del f[k]
print(json.dumps(f)[1:-1])
"#,
        ])
        .output()
        .unwrap();
    assert!(
        prefix.status.success(),
        "{}",
        String::from_utf8_lossy(&prefix.stderr)
    );
    let line = channel::format_record(
        std::str::from_utf8(&prefix.stdout).unwrap().trim(),
        &fixture(),
    );
    assert!(line.starts_with("GT710_GR {"));
    assert_eq!(line.lines().count(), 1);
    let mut scorer = Command::new("python3")
        .current_dir(root)
        .args([
            "-I",
            "-c",
            r#"
import json,runpy,sys,tempfile
from pathlib import Path
api=runpy.run_path('tests/gt710_graphics_report_test.py')
line=sys.stdin.read();e=json.loads(line.removeprefix('GT710_GR '));f=api['fixture']()
assert e==f,(e,f)
with tempfile.TemporaryDirectory() as d:
 p=Path(d);api['stage'](p,e)
 api['API']['graphics_engine'](api['API']['capture_serial'](p))
 print('GT710_GR formatter: U1 comparator accepted complete fixture')
"#,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    scorer
        .stdin
        .take()
        .unwrap()
        .write_all(line.as_bytes())
        .unwrap();
    let output = scorer.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
