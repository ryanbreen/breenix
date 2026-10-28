# Retained source-built golden saves

`captures.json` records the image-row bulk SHA-256 from each capture's serial
anchor and each decompressed image's SHA-256. The `.bin.zlib` files contain
only the decoded `GT710_GR_IMAGE` rows, compressed with zlib. They are saves
of the source-built MIT nouveau context firmware, not proprietary firmware
or shader captures.

The 191735 capture contains both the 0x55 and 0xAA images. The 195818 capture
contains only 0x55: its check aborted before another save. The map facts in
the JSON are the `GT710_GR_LAYOUT` row from 195818; replay applies that map
to each retained image. This does not turn either failed boot into an
accepted graphics run or synthesize the missing complementary image.

Run `python3 tests/gt710_save_map_test.py` to exercise the production scorer's
map statements against the retained bytes. The check requires exact byte
coverage and separately changes a written byte without consulting
image hashes. The fixture does not encode an expected gap list.

Firmware size sources (MIT, Copyright 2011 Red Hat Inc.; full notices remain
in `vendor/kepler/nouveau/`): `com.fuc:152-171,314-333`,
`hub.fuc:134-165`, `gpc.fuc:188-239`. These specify MMIO word counts,
strand size readout and 256-byte section/strand allocation spacing.
