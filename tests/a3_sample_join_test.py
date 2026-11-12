"""Exercise the offline trace/sample join; no VM or capture is needed."""
import importlib.util
from pathlib import Path
import unittest


class SampleJoin(unittest.TestCase):
    def test_counts_use_disjoint_stacks_and_join_does_not_borrow_samples(self):
        path = Path(__file__).resolve().parents[1] / 'scripts/analyze-a3-sample-join.py'
        spec = importlib.util.spec_from_file_location('a3_sample_join', path)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        graph = '''Call graph:
    10 Thread_42
    + 10 hvf_cpu_thread_fn  (in qemu)
    +   4 bql_lock_impl  (in qemu)
    +     4 __psynch_mutexwait  (in system)
    +   6 hv_vcpu_run  (in system)
    10 Thread_99
    + 10 qemu_main_loop  (in qemu)
    +   7 vrend_draw_vbo  (in virgl)
    +     5 gl::Program::resolveLinkImpl  (in ANGLE)
    +       5 MainLinkLoadEvent::wait  (in ANGLE)
    +   3 main_loop_wait  (in qemu)
Total number in stack:
'''
        result = module.sample_counts(graph)
        self.assertEqual(result['42']['states'], {'bql': 4, 'guest': 6})
        self.assertEqual(result['99']['states'], {'link': 5, 'draw': 2, 'other': 3})
        self.assertEqual(result['99']['total'], 10)
        trace = [
            '1@10.000000:virtio_gpu_cmd_ctx_submit ctx 0x15b, size 19660',
            '1@10.000010:virtio_queue_notify device',
            '1@10.200010:virtio_gpu_fence_ctrl fence',
            '1@10.300010:virtio_gpu_fence_resp fence',
        ]
        gaps = module.trace_gaps(trace, 10_000_000_000, 11_000_000_000)
        self.assertEqual(len(gaps), 1, 'strictly greater than 100 ms')
        self.assertEqual(gaps[0]['gpu_line'], 1)
        self.assertEqual(gaps[0]['duration_ns'], 200_000_000)
        windows = [{'id': 'cover', 'begin_ns': 9_000_000_000, 'end_ns': 10_250_000_000},
                   {'id': 'later', 'begin_ns': 10_250_000_000, 'end_ns': 11_000_000_000}]
        self.assertEqual(module.covering_samples(gaps[0], windows), ['cover'])
        self.assertEqual(module.covering_samples(gaps[0], windows[1:]), [])


if __name__ == '__main__':
    unittest.main()
