"""Checks replay execution without starting an emulator or connector."""
import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

import monkey


class ReplayExecution(unittest.TestCase):
    def run_expired(self, replay_file):
        with tempfile.TemporaryDirectory() as directory:
            run = monkey.Run.__new__(monkey.Run)
            run.args = SimpleNamespace(replay_file=replay_file, hw="v4", chunk=100)
            run.dir = Path(directory)
            run.image = (Path("image.bin"), Path("firmware.elf"))
            run.ports = {"uart1": 1234}
            run.seed = 1
            run.actions = [[0, "wait", 0], [0, "expect", "err", 0]]
            run.executed = []
            run.server = Mock()
            for method in ("boot", "probe", "check_alive", "state", "check_server"):
                setattr(run, method, Mock())
            run.do = run.executed.append
            with patch.object(monkey, "Emu"), patch.object(monkey, "Stackmat"):
                self.assertIsNone(run.execute(deadline=-1))
            record = json.loads((run.dir / "actions.json").read_text())
            run.server.stop.assert_called_once()
            return record

    def test_replay_finishes_assertions_after_fuzz_deadline(self):
        record = self.run_expired("delegate-late.json")
        self.assertEqual(record["executed"], record["actions"])

    def test_fuzz_stops_at_deadline(self):
        self.assertEqual(self.run_expired(None)["executed"], [])

    def test_recovery_waits_for_connection_before_tapping_a_card(self):
        run = monkey.Run.__new__(monkey.Run)
        run.stackmat = Mock()
        run.wait_for = Mock()
        run.check_alive = Mock()
        run.press = Mock()
        ready = dict(monkey.IDLE, msg="0")
        run.state = Mock(side_effect=[dict(ready, srv="Some(false)"), ready, ready])
        with patch.object(monkey.time, "sleep"):
            self.assertEqual(run.recover(), ready)
        self.assertEqual(run.state.call_count, 3)
        run.press.assert_not_called()


if __name__ == "__main__":
    unittest.main()
