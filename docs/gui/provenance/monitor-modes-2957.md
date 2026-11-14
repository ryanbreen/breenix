# Monitor mode landing review

PR [#2957](https://github.com/ryanbreen/breenix/pull/2957) derives the native catalogue
from EDID and retains the virtual panel pair.

- The BreenGL count finding is repaired in `6b806b4e`: populated wire slots are counted independently of `MODES.count`, and malformed catalogues are rejected.
- The runtime rollback finding is repaired in `488b8695`: the failed reporting window ends before the previous raster is restored and freshly checked, while the original switch error is returned.
- The console Major is rebutted by coordinator ruling 257: `20261114T184540Z-metal` is accepted at 78/88 because its ten #2572 failures match the pre-PR `20261029T091113Z-metal` signature and run unchanged serial/input source.
- The ARM Major is closed by `20261114T194357Z-qemu-46664` at `958b8c24`: GPU 170/170 and BreenGL 401/401 pass exact pixel checks and existing bounds, MEMORY passes at 16/48 and 15/48 pages, and the visible Vigil-recorded GL boot ends with exit 0 and accepted renderer attestation.
- The ARM SET_MODE race Minor is rebutted by `display.rs::set_mode`: native activation is x86-only, and ARM mode publication shares the ownership-recheck critical section.
- The scale-2 title and fixture geometry are corrected in `958b8c24`; native full-screen coverage, host/kernel catalogue parity, receiver budget admission and bulk serial diagnostics remain together in [#2991](https://github.com/ryanbreen/breenix/issues/2991).

The two targeted host checks fail on `721bdecb` and pass after their repairs:
`breengel_mode_count_test.py` and `gt710_runtime_raster_test.py`.
The x86 kernel and changed suite binaries build without warning/error lines;
ARM builds retain only the pre-existing core future-compatibility notice #2353.
No Tier-1/Tier-2 file or case threshold changes in these review repairs.

Native acceptance at `958b8c24`: GPU `20261114T193355Z-metal` passes 170/170
with MEMORY PASS 17/48; BreenGL `20261114T195552Z-metal` passes 401/401 with
MEMORY PASS 18/48. Both boots have complete native uploads, full clocks,
matching source/build/tools commits, exit 0 and confirmed power-off; neither
exports a corrected miss block, GR refusal, screen error or upload refusal.

Retained proofs: bundle `20261114T182958Z-metal`, cold readback
`20261114T184113Z-metal`, console `20261114T184540Z-metal` under ruling 257,
panic oracle `20261114T190315Z-metal`, and selected UI `20261114T112902Z-metal`.
The native source repair adds work only after a failed raster transaction;
it does not change their successful paths, diagnostics or workloads.
