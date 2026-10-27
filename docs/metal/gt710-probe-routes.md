# GT 710 probe routes

The coordinator's 2026-10-27 deadline redirect requires M6.4 console acceptance
within 330 seconds of handoff, without raising the 420-second driver deadline.

Boot `20261027T103300Z-metal` at `0aeef171` retained line offsets in `serial.ts`
beside `serial.txt`. Their intervals were inventory 59.7 s, VM/lifetime 61.0 s,
apertures 62.2 s, channel about 0.5 s, and CE/completion 1.8 s. VM and lifetime
were not timed separately, nor were CE and completion. Console START-to-DONE
was 129.8 s; handoff-to-DONE was 396.6 s. These are the earlier boot's timings,
not acceptance of the new step.

The CPU-only inventory, VM, aperture and lifetime records now run through
`metal-run boot --mode probe`, and their existing host comparisons run against
that retained capture. Console suite boots keep display, channel, CE and
completion evidence and the new context/lifetime work. The route check requires
the union of the two routes to retain the category set, without overlap.
`GT710_PROBE_TIME` reports device-side elapsed microseconds around the probes;
VM additionally reports its nested CPU lifetime interval. The CPU snapshots,
boundary checks, independent views, fault records and allocator comparisons
retain their existing criteria.

The dedicated probe boot and console acceptance use the same final commit and
its deployed tools. Score each capture once with `gt710-suite --boot ID --sha SHA`.
The console route must finish within 330 s including the new context cases.
The separate probe route keeps the existing probe-mode deadline.

The same baseline's compressed display rows occupied 553,772 wire bytes:
261,034 native-mode bytes (22.659 s at 115200 baud), 94,414 switching bytes
(8.196 s), and 198,324 flip bytes (17.216 s). CPU-memory output was only 15,487
wire bytes (1.344 s). These baud estimates assume ten bits per byte and count
CRLF; emitting bytes and CPU work may overlap, so subtracting wire time is an
estimate of non-serial work. The measured split is retained in the effort ledger.

Actual compressed rows now stay in a bounded kernel buffer and go from the
root probe/console runner to the existing boot service over TCP. The private
site file supplies the endpoint; the upload is bound to a per-boot random
route and the powered machine lease, capped at 2 MiB, checked by complete
length and SHA-256, and committed off-box before an HTTP success response.
The guest prints `GT710_BULK` length/digest on serial after success. The host
requires the matching `gt710-pixels.rle`, then feeds its original rows to the
unchanged exact pixel/state comparators and reconstructs the retained images.
Missing or corrupted bulk data fails the capture. Verdicts, native identities,
completion records and hashes stay on serial. No later disk boot is needed to
recover readback data. HTTP framing follows RFC 9112 section 6.3.
