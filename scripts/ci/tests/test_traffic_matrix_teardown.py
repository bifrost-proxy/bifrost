"""Traffic matrix isolation contract, without real proxy processes or signals."""
from __future__ import annotations

import contextlib
import importlib.util
import io
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parents[3]
MATRIX = ROOT / "e2e-tests/tests/test_traffic_search_matrix.py"
DISABLE_HELPER = "BIFROST_SYSTEM_PROXY_DISABLE_LIFECYCLE_HELPER"


class TrafficMatrixTeardownTests(unittest.TestCase):
    def exercise_fixture(self, *, inherited_flag=None, failure=False, recorded_failure=False, force_kill=False):
        env = {"SKIP_BUILD": "true", "RESULT_FILE": ""}
        if inherited_flag is not None:
            env[DISABLE_HELPER] = inherited_flag
        with mock.patch.dict(os.environ, env), tempfile.TemporaryDirectory() as tmp:
            if inherited_flag is None:
                os.environ.pop(DISABLE_HELPER, None)
            spec = importlib.util.spec_from_file_location("traffic_matrix_teardown", MATRIX)
            matrix = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(matrix)
            matrix.ROOT = Path(tmp)
            process = mock.Mock(pid=424242)
            process.wait.side_effect = (
                [subprocess.TimeoutExpired("fixture-proxy", 15), 0] if force_kill else [0]
            )
            events = []
            data_dir = None

            def launch(command, **kwargs):
                nonlocal data_dir
                data_dir = Path(kwargs["env"]["BIFROST_DATA_DIR"])
                self.assertTrue(data_dir.is_dir())
                self.assertEqual(kwargs["env"][DISABLE_HELPER], "1")
                self.assertEqual(kwargs["env"]["BIFROST_DISABLE_TRAY"], "1")
                self.assertEqual(kwargs["env"]["BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT"], "1")
                self.assertIn("--no-system-proxy", command)
                self.assertTrue(kwargs["start_new_session"])
                events.append("launch")
                return process

            def exercise(*_args):
                events.append("exercise")
                if failure:
                    raise RuntimeError("traffic assertion failed")
                matrix.RESULTS.append({"name": "fixture", "ok": not recorded_failure})

            def send_signal(pid, kind):
                # This is a mock: the sentinel PID never reaches the OS.
                self.assertEqual(pid, process.pid)
                self.assertTrue(data_dir.is_dir())
                events.append(kind)

            server = mock.Mock(server_port=43211)
            response = mock.Mock(status=200)
            connection = mock.Mock()
            connection.getresponse.return_value = response
            with (
                mock.patch.object(matrix.http.server, "ThreadingHTTPServer", return_value=server),
                mock.patch.object(matrix.threading, "Thread", return_value=mock.Mock()),
                mock.patch.object(matrix.http.client, "HTTPConnection", return_value=connection),
                mock.patch.object(matrix, "free_port", return_value=43212),
                mock.patch.object(matrix.subprocess, "Popen", side_effect=launch) as popen,
                mock.patch.object(matrix.subprocess, "run") as build,
                mock.patch.object(matrix.os, "killpg", side_effect=send_signal),
                mock.patch.object(matrix, "exercise", side_effect=exercise),
                contextlib.redirect_stdout(io.StringIO()),
            ):
                if failure:
                    with self.assertRaisesRegex(RuntimeError, "traffic assertion failed"):
                        matrix.main()
                else:
                    with self.assertRaises(SystemExit) as result:
                        matrix.main()
                    self.assertEqual(result.exception.code, recorded_failure)

            build.assert_not_called()
            popen.assert_called_once()
            self.assertFalse(data_dir.exists(), "temporary data must be removed after teardown")
            self.assertTrue(popen.call_args.kwargs["stdout"].closed)
            server.shutdown.assert_called_once_with()
            server.server_close.assert_called_once_with()
            expected = ["launch", "exercise", signal.SIGTERM]
            waits = [mock.call(timeout=15)]
            if force_kill:
                expected.append(signal.SIGKILL)
                waits.append(mock.call(timeout=5))
            self.assertEqual(events, expected)
            self.assertEqual(process.wait.call_args_list, waits)

    def test_standalone_and_inherited_enable_cannot_start_independent_writer(self):
        # Covers direct Python/Rust integration entrypoints, which do not inherit
        # scripts/run_all_e2e.sh's otherwise equivalent isolation default.
        for inherited in (None, "0", "false", "1"):
            with self.subTest(inherited=inherited):
                self.exercise_fixture(inherited_flag=inherited)

    def test_traffic_failure_still_propagates_after_owned_process_teardown(self):
        self.exercise_fixture(inherited_flag="0", failure=True)

    def test_recorded_traffic_failure_still_returns_failure_after_teardown(self):
        self.exercise_fixture(inherited_flag="0", recorded_failure=True)

    def test_owned_proxy_timeout_still_escalates_and_reaps_before_removal(self):
        self.exercise_fixture(inherited_flag="0", force_kill=True)

    def test_dedicated_lifecycle_suites_still_enable_and_exercise_helper(self):
        system = (ROOT / "e2e-tests/tests/test_system_proxy_e2e.sh").read_text()
        cli = (ROOT / "e2e-tests/tests/test_cli_proxy_environment_e2e.sh").read_text()
        self.assertIn(f"unset {DISABLE_HELPER}", cli)
        crash_case = system.split("test_lifecycle_helper_cleans_after_parent_crash() {", 1)[1]
        crash_case = crash_case.split("\n}", 1)[0]
        self.assertIn(f"unset {DISABLE_HELPER}", crash_case)
        self.assertIn("start_proxy_with_system_proxy", crash_case)
        self.assertIn('kill_pid_force "$PROXY_PID"', crash_case)
        self.assertIn("    test_lifecycle_helper_cleans_after_parent_crash\n", system)


if __name__ == "__main__":
    unittest.main()
