"""Compare a guest's boot UTC epoch with the execution host when received.

Observe during the boot, never compare an archived boot with today's clock.
The JSON sidecar keeps the original host sample separate from guest serial.
"""
import json
from pathlib import Path
import re
import time

MARKER = re.compile(r"^\[clock\] UTC epoch=(\d+) source=(CMOS|PL031|UEFI)\r?$", re.M)
MAX_OFFSET_SECONDS = 5


class WallClockCheck:
    def __init__(self, serial):
        self.sidecar = Path(str(serial) + ".utc.json")
        self.sample = None

    def observe(self, serial, host_epoch=None):
        # Only complete, unprefixed records from the clock initializer count.
        matches = list(MARKER.finditer(serial[:serial.rfind("\n") + 1]))
        if not matches:
            return
        if self.sample is not None:
            if len(matches) != 1:
                self.sample["passed"] = False
                self.sample["error"] = "duplicate UTC epoch records"
                self.sidecar.write_text(json.dumps(self.sample) + "\n")
            return
        host = time.time() if host_epoch is None else host_epoch
        guest = int(matches[0][1])
        offset = guest - host
        self.sample = dict(guest_epoch=guest, host_epoch=host,
                           offset_seconds=offset, source=matches[0][2],
                           passed=abs(offset) <= MAX_OFFSET_SECONDS)
        if len(matches) != 1:
            self.sample["passed"] = False
            self.sample["error"] = "duplicate UTC epoch records"
        self.sidecar.write_text(json.dumps(self.sample) + "\n")
        print(self.verdict()[1], flush=True)

    def verdict(self):
        if self.sample is None:
            return False, "UTC clock FAIL: missing guest UTC epoch record"
        sample = self.sample
        return sample["passed"], (
            f"UTC clock {'PASS' if sample['passed'] else 'FAIL'} "
            f"source={sample['source']} guest={sample['guest_epoch']} "
            f"host={sample['host_epoch']:.3f} offset={sample['offset_seconds']:+.3f}s "
            f"limit={MAX_OFFSET_SECONDS}s" +
            (f" ({sample['error']})" if "error" in sample else ""))
