"""Force an unfinished host renderer call while a second device needs BQL.

Pass the recipe's patched source tree as QEMU_GL_SOURCE. No guest executes.
The baseline takes its call expression from the pinned SUBMIT_3D handler; a
patched tree additionally supplies the worker implementation used by the recipe.
"""
import os
from pathlib import Path
import shlex
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SOURCE = Path(os.environ['QEMU_GL_SOURCE'])


class RendererIsolation(unittest.TestCase):
    def test_unfinished_renderer_does_not_block_other_device_and_drains_in_order(self):
        submit = (SOURCE / 'hw/display/virtio-gpu-virgl.c').read_text()
        start = submit.index('static void virgl_cmd_submit_3d(')
        end = submit.index('\nstatic void ', start + 1)
        self.assertIn('virgl_renderer_submit_cmd(buf, cs.hdr.ctx_id, cs.size / 4)',
                      submit[start:end])
        worker = SOURCE / 'hw/display/breenix-render-worker.h'
        # These are the real dispatch alternatives, not timing simulations.
        implementation = '#include "breenix-render-worker.h"' if worker.exists() else ''
        dispatch = 'virgl_cmd_submit_3d(&gpu, &cmd);'
        drain = '''
            brw_submit(w, &last);
            brw_stop(w);
            brw_complete(w);
            g_assert_cmpint(state.retired, ==, 2);
            g_assert_cmpint(state.freed, ==, 1);
            brw_free(w);
        ''' if worker.exists() else ''
        setup = '''
            w = brw_new(notify, NULL);
            BRWJob last = { .run = reset, .complete = retired_reset, .opaque = &state };
        ''' if worker.exists() else ''
        adapters = r'''
#include <stdint.h>
#include <string.h>
#include <sys/uio.h>
typedef struct {
    struct { int conf; } parent_obj;
    struct { int req_3d, bytes_3d; } stats;
} VirtIOGPU;
struct virtio_gpu_ctrl_command {
    struct { struct iovec *out_sg; int out_num; } elem;
    int error;
};
struct virtio_gpu_cmd_submit { struct { uint32_t ctx_id; } hdr; uint32_t size; };
#define VIRTIO_GPU_FILL_CMD(cs) do { (cs).hdr.ctx_id = 1; (cs).size = 4; } while (0)
#define VIRTIO_GPU_MAX_CMD_SUBMIT_SIZE (4 * 1024 * 1024)
#define VIRTIO_GPU_RESP_ERR_INVALID_PARAMETER 1
#define LOG_GUEST_ERROR 0
#define qemu_log_mask(...) ((void)0)
#define trace_virtio_gpu_cmd_ctx_submit(...) ((void)0)
static bool virtio_gpu_stats_enabled(int conf) { (void)conf; return false; }
static size_t iov_to_buf(struct iovec *iov, int n, size_t off, void *buf, size_t size) {
    (void)iov; (void)n; (void)off; memset(buf, 0, size); return size;
}
static GHashTable *breenix_attest_submitted;
static void breenix_attest(const char *event, uint32_t id) { (void)event; (void)id; }
static State *active_state;
''' + (r'''
static BRW *w;
static void submit_complete(void *opaque) { retire(opaque); }
static int virgl_renderer_submit_cmd(void *buf, int ctx, int ndw) {
    (void)buf; (void)ctx; (void)ndw;
    static BRWJob job;
    job = (BRWJob){ .run = render, .complete = submit_complete, .opaque = active_state };
    brw_submit(w, &job);
    return 0;
}
''' if worker.exists() else r'''
static int virgl_renderer_submit_cmd(void *buf, int ctx, int ndw) {
    (void)buf; (void)ctx; (void)ndw;
    active_state->main_ready = true;
    render(active_state);
    return 0;
}
''')
        program = r'''
#include <glib.h>
#include <pthread.h>
#include <stdbool.h>
''' + implementation + r'''
typedef struct {
    pthread_mutex_t bql, gate;
    pthread_cond_t cv;
    bool entered, release, mmio_progress, main_ready;
    int rendered, retired, freed;
} State;
static void render(void *opaque) {
    State *s = opaque;
    pthread_mutex_lock(&s->gate);
    s->entered = true;
    pthread_cond_broadcast(&s->cv);
    while (!s->release) pthread_cond_wait(&s->cv, &s->gate);
    s->rendered++;
    pthread_mutex_unlock(&s->gate);
}
static void *mmio(void *opaque) {
    State *s = opaque;
    pthread_mutex_lock(&s->gate);
    while (!s->entered || !s->main_ready) pthread_cond_wait(&s->cv, &s->gate);
    // A handshake, not a wall-clock timeout: the shader cannot finish yet.
    if (pthread_mutex_trylock(&s->bql) == 0) {
        s->mmio_progress = true;
        pthread_mutex_unlock(&s->bql);
    }
    s->release = true;
    pthread_cond_broadcast(&s->cv);
    pthread_mutex_unlock(&s->gate);
    return NULL;
}
''' + (r'''
static void notify(void *opaque) { (void)opaque; }
static void retire(void *opaque) {
    State *s = opaque;
    g_assert_cmpint(s->rendered, ==, 1);
    g_assert_cmpint(s->retired, ==, 0);
    s->retired++;
}
static void reset(void *opaque) {
    State *s = opaque;
    g_assert_cmpint(s->rendered, ==, 1);
    s->freed++;
}
static void retired_reset(void *opaque) {
    State *s = opaque;
    g_assert_cmpint(s->retired, ==, 1);
    s->retired++;
}
''' if worker.exists() else '') + adapters + submit[start:end] + r'''
int main(void) {
    State state = { .bql = PTHREAD_MUTEX_INITIALIZER,
                    .gate = PTHREAD_MUTEX_INITIALIZER,
                    .cv = PTHREAD_COND_INITIALIZER };
    pthread_t probe;
    active_state = &state;
    VirtIOGPU gpu = {0};
    struct virtio_gpu_ctrl_command cmd = {0};
''' + setup + r'''
    pthread_mutex_lock(&state.bql);
    g_assert_cmpint(pthread_create(&probe, NULL, mmio, &state), ==, 0);
''' + dispatch + r'''
    pthread_mutex_unlock(&state.bql);
    pthread_mutex_lock(&state.gate);
    state.main_ready = true;
    pthread_cond_broadcast(&state.cv);
    pthread_mutex_unlock(&state.gate);
    g_assert_cmpint(pthread_join(probe, NULL), ==, 0);
''' + drain + r'''
    if (!state.mmio_progress) {
        g_printerr("FAIL: unfinished SUBMIT_3D renderer holds BQL; unrelated MMIO blocked\n");
        return 1;
    }
    g_print("PASS: MMIO progressed before renderer release; ordered completion and drain\n");
    return 0;
}
'''
        with tempfile.TemporaryDirectory(prefix='a3-render-worker-') as directory:
            c = Path(directory) / 'forcing.c'
            exe = Path(directory) / 'forcing'
            c.write_text(program)
            flags = shlex.split(subprocess.check_output(
                ['pkg-config', '--cflags', '--libs', 'glib-2.0'], text=True))
            subprocess.run(['cc', '-std=c11', '-Wall', '-Wextra', '-Werror',
                            '-I' + str(worker.parent), str(c), '-o', str(exe),
                            *flags, '-pthread'], check=True)
            subprocess.run([str(exe)], check=True, timeout=20)


if __name__ == '__main__':
    unittest.main()
