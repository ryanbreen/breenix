"""Check the host trace adapter's QMP exchange without launching a VM."""
import importlib.util
import io
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("gpu_trace", ROOT / "scripts/gpu-queue-trace-qemu.py")
trace = importlib.util.module_from_spec(spec)
spec.loader.exec_module(trace)


class Stream:
    def __init__(self, replies):
        self.input = io.StringIO("\n".join(json.dumps(r) for r in replies) + "\n")
        self.output = io.StringIO()

    def readline(self):
        return self.input.readline()

    def write(self, text):
        return self.output.write(text)

    def flush(self):
        pass


class HostTrace(unittest.TestCase):
    def test_existing_events_are_enabled_outside_the_qos_launcher(self):
        stream = Stream([{"QMP": {}}, {"return": {}}, {"return": ""},
                         {"return": {"qemu": {"major": 10}}}]
                        + [{"return": {}} for _ in trace.EVENTS])
        self.assertEqual(trace.enable(stream, Path("/tmp/trace.log")), {"qemu": {"major": 10}})
        commands = [json.loads(s) for s in stream.output.getvalue().splitlines()]
        self.assertEqual(commands[:3], [
            {"execute": "qmp_capabilities"},
            {"execute": "human-monitor-command", "arguments": {"command-line": 'logfile "/tmp/trace.log"'}},
            {"execute": "query-version"},
        ])
        self.assertEqual(commands[3:], [
            {"execute": "trace-event-set-state", "arguments": {"name": event, "enable": True}}
            for event in trace.EVENTS
        ])

    def test_missing_host_event_is_a_capture_failure(self):
        stream = Stream([{"QMP": {}}, {"return": {}}, {"return": ""}, {"return": {}},
                         {"error": {"class": "GenericError", "desc": "unknown event"}}])
        with self.assertRaisesRegex(RuntimeError, "unknown event"):
            trace.enable(stream, Path("/tmp/trace.log"))


if __name__ == "__main__":
    unittest.main()
