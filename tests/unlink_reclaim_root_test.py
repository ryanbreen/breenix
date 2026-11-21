"""E24 ledger 531(t), amended by 533: preserve ext2 and route BreenFS release."""
import hashlib
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]


class UnlinkReclaimRootTest(unittest.TestCase):
    def test_root_dispatch_preserves_ext2_body(self):
        source = (ROOT / "userspace/programs/src/suite_files_io.rs").read_text()
        case = source.split("fn descriptors_unlink_reclaim() -> CaseResult {", 1)[1]
        case = case.split("\nfn descriptors_dup_clear_cloexec", 1)[0]
        dispatch, ext2 = case.split("    // The allocator hands out", 1)
        self.assertIn("if !root_contract()?.reuses_inode_numbers", dispatch)
        self.assertIn("return descriptors_unlink_reclaim_breenfs();", dispatch)
        self.assertEqual(hashlib.sha256(("    // The allocator hands out" + ext2).encode()).hexdigest(),
                         "3dc38d6be412473e2a77ebbe69aa1905e98afbd9714d234c65835d772d92abdc")


if __name__ == "__main__":
    unittest.main()
