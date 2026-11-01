# Upload cost attribution for 24.7

G24 asks for a measured synchronous-path improvement. This unit takes the
brief's instrumentation alternative: the retained capture cannot identify
the dominant remaining upload phase at the current head.

## Retained evidence

Capture `20261031T074405Z-metal` at `fc911ff3` reports
`upload-us=235484`, against the shared 100000 us bound for 16 MiB. The
interval includes the upload, the following COPY draw and wait, and the
16x16 corner readback. The case and its visibility check are unchanged.

The final-upload and case-heap bodies were checked against their serial
lengths and SHA-256 anchors. Subtracting boundary 166 from 167 gives:

| Existing cumulative slot | Calls | Time (ns) |
| --- | ---: | ---: |
| upload | 1 | 125432057 |
| read | 1 | 101261187 |
| submit | 3 | 3592345 |
| wait | 3 | 28544376 |

These are whole-case deltas, including setup; they are not a partition of
the timed visibility interval. The capture has only the first 24 cost
words, through `evo_ns`. It does not contain `user_copy`, `upload_convert`,
`transfer_map` or `ce`. The branch has since added those slots and changed
the bulk CE byte-mode path in `copy.rs`, and the readback mapping has also
changed. Assigning the old upload time to host copying, waits or CE at the
new head would be a guess.

## Additional measurements

Append two call/ns pairs to the existing record:

- `upload_stage`: `Staging::new`, including quota charging and frame allocation.
- `upload_acquire`: the resource acquisition wait, ending before its result is handled.

`user_copy` retains its inclusive meaning: allocation plus copying the
clipped user rows. Its time minus `upload_stage` isolates the user-row
copy loop. `upload_convert` measures the second host pass and cache
maintenance; `transfer_map` and `ce` retain mapping and completion costs.
Nested slots must not be summed as independent costs. Failed allocation
and interrupted acquisition still close their phase guards.

The new pairs travel through the proc record, shared ABI and compact
G12 final-upload body. The host requires the current field set for
performance acceptance; the retained 8.6 negative-control layout remains
readable without inventing missing measurements. Serial still carries
only the bulk digest. No transfer sizing, waits, fences, pixels or suite
bounds change in this unit. A later acceptance capture is needed to
attribute the remaining cost and assess the 100 ms bound.
