"""Compile the configuration constants and Linux sysinfo member interface."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ConfigurationHeaders(unittest.TestCase):
    def test_configuration_queries_and_sysinfo_fields(self):
        source = r'''
#define _POSIX_C_SOURCE 202405L
#include <unistd.h>
#include <limits.h>
#include <sys/sysinfo.h>
#include <stddef.h>
_Static_assert(LINK_MAX == 32000, "ext2 link limit");
_Static_assert(sizeof(struct sysinfo) == 112, "sysinfo ABI size");
_Static_assert(offsetof(struct sysinfo, pad) == 82, "Linux sysinfo pad");
_Static_assert(offsetof(struct sysinfo, _f) == 108, "Linux sysinfo tail");
int queries[] = {
    _PC_LINK_MAX, _PC_MAX_CANON, _PC_MAX_INPUT, _PC_NAME_MAX,
    _PC_PATH_MAX, _PC_PIPE_BUF, _PC_CHOWN_RESTRICTED, _PC_NO_TRUNC,
    _PC_VDISABLE, _PC_FILESIZEBITS, _PC_2_SYMLINKS, _PC_ALLOC_SIZE_MIN,
    _PC_REC_INCR_XFER_SIZE, _PC_REC_MAX_XFER_SIZE, _PC_REC_MIN_XFER_SIZE,
    _PC_REC_XFER_ALIGN, _PC_SYMLINK_MAX, _PC_TEXTDOMAIN_MAX, _PC_ASYNC_IO,
    _PC_FALLOC, _PC_PRIO_IO, _PC_SYNC_IO, _PC_TIMESTAMP_RESOLUTION,
    _CS_PATH, _CS_V8_ENV, _CS_V7_ENV,
    _CS_POSIX_V8_LP64_OFF64_CFLAGS, _CS_POSIX_V8_LP64_OFF64_LDFLAGS,
    _CS_POSIX_V8_LP64_OFF64_LIBS, _CS_POSIX_V8_WIDTH_RESTRICTED_ENVS,
    _CS_POSIX_V7_THREADS_CFLAGS, _CS_POSIX_V7_THREADS_LDFLAGS
};
'''
        with tempfile.TemporaryDirectory(prefix="breenix-conf-headers-") as temporary:
            path = Path(temporary) / 'queries.c'
            path.write_text(source)
            result = subprocess.run([str(ROOT / 'scripts/breenix-cc'), '--arch', 'aarch64',
                                     '-std=c17', '-Wall', '-Wextra', '-Werror', '-fsyntax-only',
                                     str(path)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(result.stderr, '')


if __name__ == '__main__':
    unittest.main()
