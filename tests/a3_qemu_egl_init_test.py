"""Run QEMU's compiled SPICE EGL initialization without a VM or a window."""
import os
from pathlib import Path
import re
import shlex
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SOURCE = Path(os.environ.get('QEMU_GL_SOURCE',
                 '/tmp/breenix-qemu-gl-build/src/qemu-10.0.12-utm'))


class QemuEGLInitialization(unittest.TestCase):
    def test_display_init_before_current_context_and_missing_optional_function(self):
        binary = Path(subprocess.check_output(
            [str(ROOT / 'scripts/build-qemu-gl.sh'), '--print-path'], text=True).strip())
        frameworks = binary.parents[1] / 'Frameworks'
        build = SOURCE / 'build-breenix'
        # Use the built enum's value and object, not a copy of the initializer.
        types = (build / 'qapi/qapi-types-ui.h').read_text()
        modes = re.search(r'typedef enum DisplayGLMode \{(.*?)\}', types, re.S)[1]
        mode_es = [s.strip() for s in modes.split(',')].index('DISPLAY_GL_MODE_ES')
        code = r'''
#include <assert.h>
#include <stdarg.h>
#include <stdio.h>
#include <string.h>
#include <sys/resource.h>
#include <epoxy/egl.h>
#include <epoxy/gl.h>

extern int qemu_egl_init_dpy_cocoa(int mode);
extern EGLDisplay *qemu_egl_display;
extern EGLContext qemu_egl_init_ctx(void);

void error_report(const char *format, ...) {
    va_list args;
    va_start(args, format);
    vfprintf(stderr, format, args);
    fputc('\n', stderr);
    va_end(args);
}

static PFNEGLGETPROCADDRESSPROC real_proc;
static unsigned blob_queries;
static __eglMustCastToProperFunctionPointerType EGLAPIENTRY
missing_blob(const char *name) {
    if (strcmp(name, "eglSetBlobCacheFuncsANDROID") == 0) {
        blob_queries++;
        return NULL;
    }
    return real_proc(name);
}

int main(int argc, char **argv) {
    /* The baseline intentionally aborts; do not write a core dump. */
    struct rlimit core = { 0, 0 };
    assert(setrlimit(RLIMIT_CORE, &core) == 0);
    assert(argc == 2);
    assert(eglGetCurrentDisplay() == EGL_NO_DISPLAY);
    real_proc = epoxy_eglGetProcAddress;
    if (strcmp(argv[1], "missing") == 0) {
        epoxy_eglGetProcAddress = missing_blob;
    }
    assert(qemu_egl_init_dpy_cocoa(MODE_ES) == 0);
    EGLDisplay display = (EGLDisplay)qemu_egl_display;
    assert(display != EGL_NO_DISPLAY);
    assert(epoxy_has_egl_extension(display, "EGL_ANDROID_blob_cache"));
    assert(!epoxy_has_egl_extension(EGL_NO_DISPLAY, "EGL_ANDROID_blob_cache"));
    assert(eglGetCurrentDisplay() == EGL_NO_DISPLAY);
    if (strcmp(argv[1], "missing") == 0) {
        assert(blob_queries == 1);
    }
    /* Proceed through the next SPICE GL step with and without the cache. */
    EGLContext context = qemu_egl_init_ctx();
    assert(context != EGL_NO_CONTEXT);
    assert(eglMakeCurrent(display, EGL_NO_SURFACE, EGL_NO_SURFACE, context));
    const char *renderer = (const char *)glGetString(GL_RENDERER);
    assert(renderer && strstr(renderer, "ANGLE") && strstr(renderer, "Metal"));
    assert(eglMakeCurrent(display, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT));
    assert(eglDestroyContext(display, context));
    assert(eglTerminate(display));
    puts("QEMU_EGL_INIT_OK");
    return 0;
}
'''.replace('MODE_ES', str(mode_es))
        with tempfile.TemporaryDirectory(prefix='a3-qemu-egl-init-') as directory:
            directory = Path(directory)
            host = directory / 'egl-init.c'
            executable = directory / 'egl-init'
            host.write_text(code)
            flags = shlex.split(subprocess.check_output(
                ['pkg-config', '--cflags', 'glib-2.0'], text=True))
            subprocess.run([
                'cc', '-std=c11', '-Wall', '-Wextra', '-Werror',
                '-I' + str(SOURCE.parents[1] / 'prefix/include'),
                str(host), str(build / 'libcommon.a.p/ui_egl-helpers.c.o'),
                str(frameworks / 'libepoxy.0.dylib'),
                str(frameworks / 'libglib-2.0.0.dylib'),
                '-Wl,-dead_strip', '-Wl,-rpath,' + str(frameworks),
                '-o', str(executable), *flags,
            ], check=True)
            env = dict(os.environ, ANGLE_DEFAULT_PLATFORM='metal')
            for name in ('ANGLE_FEATURE_OVERRIDES_ENABLED', 'ANGLE_FEATURE_OVERRIDES_DISABLED'):
                env.pop(name, None)
            for mode in ('available', 'missing'):
                result = subprocess.run([str(executable), mode], env=env,
                                        capture_output=True, text=True, timeout=30)
                print(f'{mode}: exit={result.returncode}\n{result.stdout}{result.stderr}',
                      flush=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn('QEMU_EGL_INIT_OK', result.stdout)
                if mode == 'missing':
                    self.assertIn('eglSetBlobCacheFuncsANDROID unavailable', result.stderr)
                    self.assertIn('cache disabled', result.stderr)


if __name__ == '__main__':
    unittest.main()
