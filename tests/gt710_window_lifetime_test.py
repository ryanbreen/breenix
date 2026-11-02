"""Execute production exit/exec, remnant and target retirement on modeled hardware.

The host substitutes process rows, physical pages and receipt arrival. Native
resource allocation/unref/census, quota guards, target destruction, window
teardown hooks and the suite's unchanged B12 drain loop execute from source.
This check does not certify hardware rendering or guest scheduling.
"""
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class WindowLifetimes(unittest.TestCase):
    def test_process_exit_and_exec_wire_both_cleanup_hooks_once(self):
        process = (ROOT / 'kernel/src/process/mod.rs').read_text()
        task = (ROOT / 'kernel/src/task/process_task.rs').read_text()
        handlers = (ROOT / 'kernel/src/syscall/handlers.rs').read_text()
        for source, marker, hook in (
            (process, 'pub fn exit_process_and_retire(', 'crate::window::row_exit(pid);'),
            (task, '    pub fn handle_thread_exit(', 'crate::window::thread_exit(thread_id);'),
        ):
            body = item(source, marker)
            self.assertEqual(body.count(hook), 1)
            self.assertEqual(body.count('crate::gpu::exited_locked('), 1)
            self.assertEqual(body.count('crate::gpu::owner_ended(owner);'), 1)
            self.assertLess(body.index(hook), body.index('crate::gpu::exited_locked('))
            self.assertLess(body.index('crate::gpu::exited_locked('), body.index('crate::gpu::owner_ended(owner);'))
        body = item(handlers, 'pub fn sys_execv_with_frame(')
        # The production and testing cfg arms must each clean only a committed exec.
        for call in ('crate::window::exec_done_current();', 'crate::gpu::owner_ended(old_gpu_owner);'):
            guarded = (r'if matches!\(result, SyscallResult::Ok\(0\)\) \{\s*'
                       + re.escape(call) + r'\s*\}')
            self.assertEqual(body.count(call), 2)
            self.assertEqual(len(re.findall(guarded, body)), 2)

    def test_exit_exec_remnants_and_bounded_unref(self):
        core = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        target = (ROOT / 'kernel/src/gpu/window_targets.rs').read_text()
        ops = (ROOT / 'kernel/src/window/ops.rs').read_text()
        window = (ROOT / 'kernel/src/window/mod.rs').read_text()
        native = (ROOT / 'kernel/src/gpu/gk208/resource.rs').read_text()
        fixture = (ROOT / 'tests/fixtures/gt710_window_lifetime.rs').read_text()
        fixture = fixture.replace('// CORE_LIFETIME', '\n'.join(item(core, m) for m in (
            '    fn unhold(', '    fn free_if_unused(', '    fn retire_if_unused(', '    fn reclaim(')))
        for marker, body in {
            '// SETTLE': item(core, '    fn settle('),
            '// MAPPED_BYTES': item((ROOT / 'kernel/src/process/limits.rs').read_text(), '    pub fn mapped_bytes('),
            '// CHARGED_BYTES': item(window, 'pub fn charged_bytes('),
            '// UNMAP_ALL': item(ops, 'fn unmap_all('),
            '// DESTROY_WINDOW': item(ops, 'pub fn destroy('),
            '// DESTROY_RESOURCE': item(core, 'fn destroy_resource('),
            '// DESTROY_CONTEXT': item(core, 'fn destroy_context('),
            '// BACKING_LEN': item(target, 'pub fn backing_len('),
            '// DEVICE_FRAMES': item(core, 'fn device_frames('),
            '// PITCH': 'pub(super) ' + item((ROOT / 'kernel/src/gpu/console.rs').read_text(), 'fn pitch_of('),
            '// CHARGE_COPY': item((ROOT / 'kernel/src/gpu/dma.rs').read_text(), '    pub(super) fn charge_copy('),
            '// CONTEXT': target[target.index('static CONTEXTS:'):target.index(';', target.index('static CONTEXTS:'))+1] + '\n' + '\n'.join(item(target, m) for m in ('struct Context {', 'impl Context {', 'impl Drop for Context {', 'pub fn release_context(')),
            '// TARGET_RECORD': item(target, 'pub struct Target {'),
            '// TARGET_MAKE': item(target, '    fn make('),
            '// TARGET_REVOKE': item(target, '    pub fn revoke('),
            '// TARGET_DROP': item(target, 'impl Drop for Target {'),
            '// GROUP_GONE': item(ops, 'pub fn group_gone('),
            '// ROOM_OF': item(ops, 'fn room_of('),
            '// RELEASE_REMNANT': item(ops, 'pub fn release_remnant('),
            '// REMNANT': item(window, 'pub struct Remnant {').replace('Arc<Mem>', 'Arc<Tree>'),
            '// EXIT_HOOKS': '\n'.join(item(window, m) for m in ('pub fn thread_exit(', 'pub fn row_exit(', 'pub fn exec_done_current(')),
            '// STORAGE_REQUEST': item((ROOT / 'kernel/src/gpu/gk208/adapter.rs').read_text(), 'pub struct Request {'),
            '// STORAGE_CONSTRUCTOR': item((ROOT / 'kernel/src/gpu/gk208/adapter.rs').read_text(), '    pub(crate) fn storage('),
        }.items():
            fixture = fixture.replace(marker, body)
        suite = (ROOT / 'userspace/programs/src/suite_breengel/window_cases/windows.rs').read_text()
        churn = item(suite, 'fn windows_churn(')
        drain = churn[churn.index('    let drain ='):churn.index('    drain()?;')]
        fixture = fixture.replace('// B12_DRAIN', drain)
        entry_points = '\n'.join(item(native, m) for m in ('    pub fn ctx_create(', '    pub fn ctx_destroy(', '    pub fn resource_create(',
            '    pub fn attach_backing(', '    pub fn resource_unref(', '    pub(crate) fn ownership_census('))
        with tempfile.TemporaryDirectory(prefix='gt710-lifetime-') as tmp:
            p = Path(tmp)
            resource = native.replace('#[path = "present.rs"]', f'#[path = "{ROOT / "kernel/src/gpu/gk208/present.rs"}"]')
            resource += '\nmod host_entry_points {\nuse super::*;\nuse crate::gpu::{dev::Tag,dma::Pages};\nuse crate::gk208::{ce,adapter::Request};\n' + entry_points + '\n}\npub(crate) use host_entry_points::*;\n'
            (p / 'resource.rs').write_text(resource)
            fixture = fixture.replace('// RESOURCE_MODULE', f'#[allow(dead_code)] // Full native storage module; hardware entry points are modeled.\n#[path="{p / "resource.rs"}"] pub mod resource;')
            fixture = fixture.replace('ROOT', str(ROOT)).replace('pub(super)', 'pub(crate)')
            (p / 'lifetime.rs').write_text(fixture)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', '--crate-name', 'breengel_abi',
                '--crate-type', 'rlib', str(ROOT / 'libs/breengel-abi/src/lib.rs'), '-o', str(p / 'libbreengel_abi.rlib')], check=True)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', '--test', '--extern',
                f'breengel_abi={p / "libbreengel_abi.rlib"}', str(p / 'lifetime.rs'), '-o', str(p / 'lifetime')], check=True)
            subprocess.run([str(p / 'lifetime'), '--test-threads=1'], check=True)


if __name__ == '__main__':
    unittest.main()
