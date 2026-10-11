"""Bracket a guest's boot UTC epoch with execution-host launch and receipt time.

Observe during the boot, never compare an archived boot with today's clock.
Host scheduling delays may extend receipt time; RTC seconds may round down.
"""
from pathlib import Path
import re
import time

MARKER = re.compile(r"^\[clock\] UTC epoch=(\d+) source=(CMOS|PL031|UEFI)\r?$", re.M)


class WallClockCheck:
    def __init__(self, serial, started_epoch=None):
        # Remove obsolete evidence from an earlier boot using the same log.
        Path(str(serial) + ".utc.json").unlink(missing_ok=True)
        self.started_epoch = time.time() if started_epoch is None else started_epoch
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
            return
        host = time.time() if host_epoch is None else host_epoch
        guest = int(matches[0][1])
        offset = guest - host
        self.sample = dict(guest_epoch=guest, host_epoch=host,
                           offset_seconds=offset, source=matches[0][2],
                           passed=self.started_epoch - 1 <= guest <= host)
        if len(matches) != 1:
            self.sample["passed"] = False
            self.sample["error"] = "duplicate UTC epoch records"
        print(self.verdict()[1], flush=True)

    def verdict(self):
        if self.sample is None:
            return False, "UTC clock FAIL: missing guest UTC epoch record"
        sample = self.sample
        return sample["passed"], (
            f"UTC clock {'PASS' if sample['passed'] else 'FAIL'} "
            f"source={sample['source']} guest={sample['guest_epoch']} "
            f"host={sample['host_epoch']:.3f} offset={sample['offset_seconds']:+.3f}s "
            f"bracket=[{self.started_epoch - 1:.3f},{sample['host_epoch']:.3f}]" +
            (f" ({sample['error']})" if "error" in sample else ""))
