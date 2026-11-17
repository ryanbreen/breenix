#!/usr/bin/env python3
"""Stand-in for `cargo test` in scripts/test_breenfs_battery.py: prints canned libtest output.

    libtest_stub.py list
    libtest_stub.py run pass|known|real
    libtest_stub.py run hang PIDFILE
"""

import os
import sys
import time

LIST = """a::one: test
a::two: test
torn_images_recover_to_an_acknowledged_commit: test

3 tests, 0 benchmarks
"""

HEAD = """   Compiling breenfs-host-tests v0.1.0
     Running tests/crash_images.rs (target/debug/deps/crash_images-0000)

running 3 tests
test a::one ... ok <0.012s>
test a::two ... ignored, run with --ignored
"""


def failed(message: str) -> str:
    return f"""test torn_images_recover_to_an_acknowledged_commit ... FAILED <601.250s>

failures:

---- torn_images_recover_to_an_acknowledged_commit stdout ----
subset corpus: some captured output

thread 'torn_images_recover_to_an_acknowledged_commit' panicked at tests/crash_images.rs:151:21:
{message}
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    torn_images_recover_to_an_acknowledged_commit

test result: FAILED. 1 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out; finished in 601.25s

error: test failed, to rerun pass `--test crash_images`
"""


def main() -> int:
    mode = sys.argv[1]
    if mode == "list":
        sys.stdout.write(LIST)
        return 0
    scenario = sys.argv[2]
    sys.stdout.write(HEAD)
    sys.stdout.flush()
    if scenario == "pass":
        # The nocapture form: the name first, the result on a later line.
        sys.stdout.write("test torn_images_recover_to_an_acknowledged_commit ... \n")
        sys.stdout.write("captured line printed by the test\n")
        sys.stdout.write("ok <12.500s>\n\ntest result: ok. 2 passed; 0 failed; 1 ignored\n")
        return 0
    if scenario == "known":
        sys.stdout.write(failed("torn corpus exceeded 10 minutes"))
        return 101
    if scenario == "real":
        sys.stdout.write(failed("assertion `left == right` failed: seed=2 tail=Old image=7\n  left: Some(1)\n right: Some(0)"))
        return 101
    if scenario == "hang":
        with open(sys.argv[3], "w", encoding="utf-8") as pidfile:
            pidfile.write(str(os.getpid()))
        time.sleep(600)
        return 0
    return 2


if __name__ == "__main__":
    sys.exit(main())
