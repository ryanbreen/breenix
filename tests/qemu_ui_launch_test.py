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
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/proc_info.h>
#include <unistd.h>

// Thread IDs of main, child (created by main) and grandchild (created by child), and the class
// the utility thread finds itself in. The threads park; main reads their priorities from outside
// them, as a host sampler reads QEMU's, because a thread blocked in pthread_join would lend the
// joining thread's priority to the one it waits for.
static uint64_t ids[3];
static volatile qos_class_t utility_class;

static int base_priority(uint64_t tid) {
    struct proc_threadinfo info;
    if (proc_pidinfo(getpid(), PROC_PIDTHREADID64INFO, tid, &info, sizeof info) != sizeof info) return -1;
    return info.pth_priority;
}
static void *park(void *slot) {
    pthread_threadid_np(NULL, &ids[(long)slot]);
    for (;;) pause();
}
static void *park_utility(void *unused) {
    utility_class = qos_class_self();
    for (;;) pause();
}
// Like QEMU's qemu_thread_create: an initialized attribute object with no class set.
static void *create_from_thread(void *unused) {
    pthread_t thread;
    pthread_attr_t attr;
    pthread_threadid_np(NULL, &ids[1]);
    pthread_attr_init(&attr);
    pthread_create(&thread, &attr, park, (void *)2);
    for (;;) pause();
}
int main(void) {
    pthread_t thread;
    pthread_attr_t attr, utility;
    const char *inserted = getenv("DYLD_INSERT_LIBRARIES");
    pthread_threadid_np(NULL, &ids[0]);
    pthread_attr_init(&attr);
    pthread_create(&thread, &attr, create_from_thread, NULL);
    pthread_attr_init(&utility);
    pthread_attr_set_qos_class_np(&utility, QOS_CLASS_UTILITY, 0);
    pthread_create(&thread, &utility, park_utility, NULL);
    usleep(300000);
    printf("pid=%d\ninserted=%s\nmain=%d\nchild=%d\ngrandchild=%d\nutility=0x%x\n", getpid(),
           inserted ? inserted : "", base_priority(ids[0]), base_priority(ids[1]), base_priority(ids[2]),
           utility_class);
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


if __name__ == "__main__":
    unittest.main()
