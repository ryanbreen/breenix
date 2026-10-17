"""Known TCP probe file, independent of the single-transfer firmware payload."""
import hashlib

PROBE_PATH = '/breenix/probe.bin'
PROBE_BODY = bytes(range(256)) * 512
PROBE_SHA256 = hashlib.sha256(PROBE_BODY).hexdigest()
