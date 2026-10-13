#!/usr/bin/env python3
"""Check scripts/qemu-ui-launch.sh's launcher on this Mac, without a VM.

A probe program creates threads the way QEMU creates its vCPU threads (from a thread that is not
the main thread, with an initialized attribute object) and prints each thread's base priority as
macOS reports it. Run directly or with
unittest discovery; what QEMU's own threads get is checked by a real boot.
"""

import os
from pathlib import Path
import platform
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
HELPER = ROOT / "scripts/qemu-ui-launch.sh"

PROBE = r"""
#include <libproc.h>
#include <pthread.h>
#include <pthread/qos.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/proc_info.h>
#include <unistd.h>

// Thread IDs of main, child (created by main) and grandchild (created by child), and the class
// the utility thread finds itself in. Each thread stores its value and then counts itself ready
// with a release store; main waits, for at most 10 s, until all three have, and reads the values
// after an acquire load. The threads park; main reads their priorities from outside them, as a
// host sampler reads QEMU's, because a thread blocked in pthread_join would lend the joining
// thread's priority to the one it waits for.
static _Atomic uint64_t ids[3];
static _Atomic qos_class_t utility_class;
static atomic_int ready;

static int base_priority(uint64_t tid) {
    struct proc_threadinfo info;
    if (proc_pidinfo(getpid(), PROC_PIDTHREADID64INFO, tid, &info, sizeof info) != sizeof info) return -1;
    return info.pth_priority;
}
static void publish_id(long slot) {
    uint64_t tid;
    pthread_threadid_np(NULL, &tid);
    atomic_store_explicit(&ids[slot], tid, memory_order_relaxed);
}
static void *park(void *slot) {
    publish_id((long)slot);
    atomic_fetch_add_explicit(&ready, 1, memory_order_release);
    for (;;) pause();
}
static void *park_utility(void *unused) {
    atomic_store_explicit(&utility_class, qos_class_self(), memory_order_relaxed);
    atomic_fetch_add_explicit(&ready, 1, memory_order_release);
    for (;;) pause();
}
// Like QEMU's qemu_thread_create: an initialized attribute object with no class set.
static void *create_from_thread(void *unused) {
    pthread_t thread;
    pthread_attr_t attr;
    publish_id(1);
    atomic_fetch_add_explicit(&ready, 1, memory_order_release);
    pthread_attr_init(&attr);
    pthread_create(&thread, &attr, park, (void *)2);
    for (;;) pause();
}
int main(void) {
    pthread_t thread;
    pthread_attr_t attr, utility;
    const char *inserted = getenv("DYLD_INSERT_LIBRARIES");
    publish_id(0);
    pthread_attr_init(&attr);
    pthread_create(&thread, &attr, create_from_thread, NULL);
    pthread_attr_init(&utility);
    pthread_attr_set_qos_class_np(&utility, QOS_CLASS_UTILITY, 0);
    pthread_create(&thread, &utility, park_utility, NULL);
    for (int waited_ms = 0; atomic_load_explicit(&ready, memory_order_acquire) < 3; waited_ms++) {
        if (waited_ms == 10000) {
            fprintf(stderr, "probe: threads not ready after 10 s\n");
            return 1;
        }
        usleep(1000);
    }
    printf("pid=%d\ninserted=%s\nmain=%d\nchild=%d\ngrandchild=%d\nutility=0x%x\n", getpid(),
           inserted ? inserted : "",
           base_priority(atomic_load_explicit(&ids[0], memory_order_relaxed)),
           base_priority(atomic_load_explicit(&ids[1], memory_order_relaxed)),
           base_priority(atomic_load_explicit(&ids[2], memory_order_relaxed)),
           atomic_load_explicit(&utility_class, memory_order_relaxed));
    return 0;
}
"""


def helper(**env):
    return subprocess.run([str(HELPER)], capture_output=True, text=True, check=True,
                          env={**os.environ, **env}).stdout.strip()


@unittest.skipUnless(platform.system() == "Darwin", "the launcher sets macOS scheduling policy")
class LauncherTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.probe = Path(cls.tmp.name) / "probe"
        source = Path(cls.tmp.name) / "probe.c"
        source.write_text(PROBE)
        subprocess.run(["cc", "-Wall", "-Werror", "-o", str(cls.probe), str(source)], check=True)
        cls.launcher = helper()

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def run_probe(self, *prefix):
        process = subprocess.Popen([*prefix, str(self.probe)], stdout=subprocess.PIPE, text=True)
        out, _ = process.communicate(timeout=30)
        self.assertEqual(process.returncode, 0)
        values = dict(line.split("=", 1) for line in out.splitlines())
        return process.pid, values

    def test_helper_prints_a_built_launcher(self):
        self.assertTrue(self.launcher.endswith("/qemu-ui-launch"), self.launcher)
        self.assertTrue(os.access(self.launcher, os.X_OK))
        self.assertTrue((Path(self.launcher).parent / "qemu-ui-threads.dylib").is_file())

    def test_default_launch_leaves_threads_at_the_default_priority(self):
        _, values = self.run_probe()
        for name in ("main", "child", "grandchild"):
            self.assertEqual(values[name], "31", name)

    def test_threads_created_by_threads_run_in_the_user_interactive_band(self):
        _, values = self.run_probe(self.launcher)
        for name in ("main", "child", "grandchild"):
            self.assertEqual(values[name], "46", name)

    def test_a_thread_created_with_its_own_class_keeps_it(self):
        _, values = self.run_probe(self.launcher)
        self.assertEqual(int(values["utility"], 16), 0x11)  # QOS_CLASS_UTILITY

    def test_the_program_keeps_the_launcher_process_id(self):
        pid, values = self.run_probe(self.launcher)
        self.assertEqual(values["pid"], str(pid))

    def test_the_program_does_not_pass_the_library_on(self):
        _, values = self.run_probe(self.launcher)
        self.assertEqual(values["inserted"], "")

    def test_a_missing_program_exits_127(self):
        result = subprocess.run([self.launcher, "/nonexistent/qemu"], capture_output=True, text=True)
        self.assertEqual(result.returncode, 127)
        self.assertIn("/nonexistent/qemu", result.stderr)

    def test_off_switch_prints_env(self):
        self.assertEqual(helper(BREENIX_QEMU_UI="0"), "env")

    def test_the_helper_runs_a_command_through_the_launcher(self):
        pid, values = self.run_probe(str(HELPER))
        self.assertEqual(values["pid"], str(pid))
        for name in ("main", "child", "grandchild"):
            self.assertEqual(values[name], "46", name)

    def test_the_off_switch_runs_a_command_unchanged(self):
        process = subprocess.Popen([str(HELPER), str(self.probe)], stdout=subprocess.PIPE, text=True,
                                   env={**os.environ, "BREENIX_QEMU_UI": "0"})
        out, _ = process.communicate(timeout=30)
        self.assertEqual(process.returncode, 0)
        values = dict(line.split("=", 1) for line in out.splitlines())
        for name in ("main", "child", "grandchild"):
            self.assertEqual(values[name], "31", name)

    def test_concurrent_cold_callers_share_one_complete_entry(self):
        with tempfile.TemporaryDirectory() as cache:
            env = {**os.environ, "BREENIX_QEMU_APP_DIR": cache}
            callers = [subprocess.Popen([str(HELPER)], stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                        text=True, env=env) for _ in range(6)]
            results = [caller.communicate(timeout=120) for caller in callers]
            paths = {out.strip() for out, _ in results}
            self.assertEqual([caller.returncode for caller in callers], [0] * 6)
            self.assertEqual([err for _, err in results], [""] * 6)
            self.assertEqual(len(paths), 1, paths)
            launcher = Path(paths.pop())
            self.assertEqual(launcher.parent.parent, Path(cache))
            self.assertTrue(os.access(launcher, os.X_OK))
            self.assertTrue((launcher.parent / "qemu-ui-threads.dylib").is_file())
            self.assertEqual(sorted(p.name for p in Path(cache).iterdir()),
                             sorted([launcher.parent.name, "qemu-ui.lock"]))

    def test_sourcing_the_host_lock_library_builds_nothing(self):
        with tempfile.TemporaryDirectory() as cache:
            env = {key: value for key, value in os.environ.items()
                   if key not in ("BREENIX_HOST_ENTRY", "BREENIX_SLOT_SESSION")}
            result = subprocess.run(
                ["bash", "-c", 'source "$1/docker/qemu/lib/qemu-host-lock.sh" && echo "$QEMU_UI"', "_", str(ROOT)],
                capture_output=True, text=True, env={**env, "BREENIX_QEMU_APP_DIR": cache})
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout.strip(), str(HELPER))
            self.assertEqual(list(Path(cache).iterdir()), [])


if __name__ == "__main__":
    unittest.main()
