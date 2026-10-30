"""Check that both x86 exec variants hand the old GPU owner to shared reclamation."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ExecOwner(unittest.TestCase):
    def test_successful_x86_exec_redeems_captured_owner_after_commit(self):
        source = (ROOT / 'kernel/src/syscall/handlers.rs').read_text()
        function = source.split('pub fn sys_execv_with_frame(', 1)[1].split('/// sys_spawn', 1)[0]
        arms = function.split('#[cfg(not(feature = "testing"))]')
        self.assertEqual(len(arms), 2)
        for arm in arms:
            with self.subTest(arm=arm[:50]):
                self.assertRegex(arm, r'let \(current_pid, image_cred, old_gpu_owner\)')
                self.assertRegex(arm, r'row\.thread_group_id\.unwrap_or\(pid\.as_u64\(\)\)')
                self.assertRegex(arm, r'let result = loop\s*\{')
                self.assertRegex(arm, r'if matches!\(result, SyscallResult::Ok\(0\)\)\s*\{\s*crate::gpu::owner_ended\(old_gpu_owner\);\s*\}')
                self.assertLess(arm.index('drop(manager_guard);'), arm.index('crate::gpu::owner_ended(old_gpu_owner);'))
                self.assertLess(arm.index('commit.apply();'), arm.index('crate::gpu::owner_ended(old_gpu_owner);'))


if __name__ == '__main__':
    unittest.main()
