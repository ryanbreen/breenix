"""Exercise the recipe's actual disk callbacks with the production shader set."""
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SOURCE = Path(os.environ.get('QEMU_GL_SOURCE',
                 '/tmp/breenix-qemu-gl-build/src/qemu-10.0.12-utm'))
PREFIX = Path(os.environ.get('QEMU_GL_PREFIX', '/tmp/breenix-qemu-gl-build/prefix'))


class HostProgramCache(unittest.TestCase):
    def test_patched_callbacks_populate_and_hit_after_restart(self):
        code = r'''
#include <stdbool.h>
#include <stdio.h>
#include <stdint.h>
#include <string.h>
#include <epoxy/egl.h>
static EGLSetBlobFuncANDROID cache_set;
static EGLGetBlobFuncANDROID cache_get;
static uint64_t stats[4];
static void record_set(const void *key, EGLsizeiANDROID n,
                         const void *value, EGLsizeiANDROID size) {
    stats[2]++; stats[3] += size; cache_set(key, n, value, size);
}
static EGLsizeiANDROID record_get(const void *key, EGLsizeiANDROID n,
                                    void *value, EGLsizeiANDROID size) {
    EGLsizeiANDROID result = cache_get(key, n, value, size);
    stats[0]++;
    if (result && value && size >= result) stats[1]++;
    return result;
}
static void record_install(EGLDisplay display, EGLSetBlobFuncANDROID set,
                            EGLGetBlobFuncANDROID get) {
    cache_set = set; cache_get = get;
    epoxy_eglSetBlobCacheFuncsANDROID(display, record_set, record_get);
}
#undef eglSetBlobCacheFuncsANDROID
#define eglSetBlobCacheFuncsANDROID record_install
#include "breenix-blob-cache.h"
void a3_cache_install(EGLDisplay display, const char *pin) {
    breenix_blob_cache_install(display, pin);
    g_assert(bbc_directory != NULL);
    /* These exercise actual disk record parsing, not a second implementation. */
    const char key[] = "a3-small-record", value[] = "cache round trip";
    char buffer[sizeof(value)];
    bbc_set(key, sizeof(key), value, sizeof(value));
    g_assert_cmpint(bbc_get(key, sizeof(key), NULL, 0), ==, sizeof(value));
    memset(buffer, 0x7f, sizeof(buffer));
    g_assert_cmpint(bbc_get(key, sizeof(key), buffer, 1), ==, sizeof(value));
    g_assert_cmpint(buffer[0], ==, 0x7f);
    g_assert_cmpint(bbc_get(key, sizeof(key), buffer, sizeof(buffer)), ==, sizeof(value));
    g_assert_cmpmem(buffer, sizeof(buffer), value, sizeof(value));
    char *path = bbc_path(key, sizeof(key));
    g_assert(g_file_set_contents(path, "corrupt", 7, NULL));
    g_assert_cmpint(bbc_get(key, sizeof(key), buffer, sizeof(buffer)), ==, 0);
    unlink(path); g_free(path);
}
uint64_t a3_cache_stat(unsigned index) { g_assert(index < 4); return stats[index]; }
'''
        with tempfile.TemporaryDirectory(prefix='a3-host-cache-') as directory:
            directory = Path(directory)
            shim = directory / 'cache.c'
            dylib = directory / 'cache.dylib'
            shim.write_text(code)
            flags = shlex.split(subprocess.check_output(
                ['pkg-config', '--cflags', '--libs', 'glib-2.0'], text=True))
            subprocess.run(['cc', '-dynamiclib', '-std=c11', '-Wall', '-Wextra', '-Werror',
                            '-I' + str(SOURCE / 'ui'), '-I' + str(PREFIX / 'include'),
                            str(shim), '-L' + str(PREFIX / 'lib'), '-lepoxy',
                            '-o', str(dylib), *flags], check=True)
            output = Path(os.environ.get('A3_HOST_CACHE_OUTPUT', directory / 'result'))
            subprocess.run([sys.executable, str(ROOT / 'scripts/measure-angle-cache.py'),
                            '--output', str(output), '--host-cache-library', str(dylib)],
                           check=True)
            result = json.loads((output / 'results.json').read_text())
            disabled, populate, persistent = result['runs']
            self.assertEqual(disabled['cache']['gets'], 0)
            self.assertGreater(populate['cache']['puts'], 0)
            self.assertGreater(persistent['cache']['hits'], 0)
            self.assertEqual(persistent['cache']['puts'], 0)
            for run in result['runs']:
                for context in run['contexts']:
                    self.assertEqual(len(context), 16)
                    self.assertTrue(all(row['linked'] for row in context))


if __name__ == '__main__':
    unittest.main()
