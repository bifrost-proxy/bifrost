"""Exercise handoff fixture readiness without starting a proxy or changing the host."""
from __future__ import annotations

import contextlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parents[3]
FIXTURE = ROOT / "e2e-tests/tests/test_proxy_recovery_handoff.py"


def load_fixture():
    spec = importlib.util.spec_from_file_location("proxy_recovery_handoff", FIXTURE)
    fixture = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(fixture)
    return fixture


class HandoffReadinessTests(unittest.TestCase):
    def setUp(self):
        self.fixture = load_fixture()
        temporary = tempfile.TemporaryDirectory(prefix="bifrost-handoff-readiness-")
        self.addCleanup(temporary.cleanup)
        self.data = Path(temporary.name)
        self.marker = self.data / "runtime.json"
        self.log = self.data / "core.log"
        self.log.write_text("fixture diagnostic")
        self.child = mock.Mock(pid=424242, returncode=None)
        self.child.poll.return_value = None
        self.port = 43211
        self.runtime = {
            "pid": self.child.pid, "host": "127.0.0.1", "port": self.port,
            "system_proxy_enabled": False, "system_proxy_config_revision": 2,
        }
        self.now = 0.0
        self.pending = []
        self.clock = mock.Mock()
        self.clock.monotonic.side_effect = lambda: self.now
        self.clock.sleep.side_effect = self.sleep
        self.canary = mock.Mock(return_value=True)
        for name, value in (("time", self.clock), ("canary", self.canary)):
            patcher = mock.patch.object(self.fixture, name, value)
            patcher.start()
            self.addCleanup(patcher.stop)

    def publish(self, value):
        if value is None:
            self.marker.unlink(missing_ok=True)
        else:
            self.marker.write_text(value if isinstance(value, str) else json.dumps(value))

    def sleep(self, delay):
        self.assertGreater(delay, 0)
        self.assertLessEqual(delay, 0.1)
        self.now += delay
        if self.pending:
            self.publish(self.pending.pop(0))

    def wait(self):
        return self.fixture.wait_for_runtime(self.port, self.data, self.child, self.log)

    def test_delayed_missing_partial_and_non_object_metadata_is_retried(self):
        # The listener may already answer while runtime.json is still unpublished
        # or being written; no sleeps or live processes are needed to reproduce it.
        self.pending = ["{", [], {}, {**self.runtime, "pid": None}, self.runtime]
        self.assertEqual(self.wait(), self.runtime)
        self.assertEqual(self.clock.sleep.call_count, 5)
        self.canary.assert_called_once_with(self.port)

    def test_every_required_field_must_be_present_before_accepting_marker(self):
        for field in self.runtime:
            with self.subTest(field=field):
                incomplete = self.runtime.copy()
                del incomplete[field]
                self.publish(incomplete)
                self.pending = [self.runtime]
                self.clock.sleep.reset_mock()
                self.assertEqual(self.wait(), self.runtime)
                self.clock.sleep.assert_called_once()

    def test_stale_child_host_and_pre_rebind_endpoint_are_retried(self):
        for field, stale in (("pid", 424241), ("host", "127.0.0.2"), ("port", 43210)):
            with self.subTest(field=field):
                self.publish({**self.runtime, field: stale})
                self.pending = [self.runtime]
                self.clock.sleep.reset_mock()
                self.assertEqual(self.wait(), self.runtime)
                self.clock.sleep.assert_called_once()

    def test_incomplete_intent_types_are_retried(self):
        for field, value in (
            ("system_proxy_enabled", None), ("system_proxy_enabled", "false"),
            ("system_proxy_enabled", 0), ("system_proxy_config_revision", None),
            ("system_proxy_config_revision", "2"), ("system_proxy_config_revision", True),
        ):
            with self.subTest(field=field, value=value):
                self.publish({**self.runtime, field: value})
                self.pending = [self.runtime]
                self.clock.sleep.reset_mock()
                self.assertEqual(self.wait(), self.runtime)
                self.clock.sleep.assert_called_once()

    def test_ready_snapshot_is_returned_without_waiting_for_expected_intent_values(self):
        # Readiness must not hide an incorrect intent/revision until a later write:
        # the caller's existing strict assertions must see the first complete value.
        for enabled, revision in ((False, 2), (True, 1), (False, 0)):
            with self.subTest(enabled=enabled, revision=revision):
                runtime = {**self.runtime, "system_proxy_enabled": enabled,
                           "system_proxy_config_revision": revision}
                self.publish(runtime)
                self.assertEqual(self.wait(), runtime)
        self.clock.sleep.assert_not_called()

    def test_marker_alone_does_not_replace_live_canary(self):
        self.publish(self.runtime)
        self.canary.side_effect = [OSError("not listening yet"), False, True]
        self.assertEqual(self.wait(), self.runtime)
        self.assertEqual(self.canary.call_count, 3)
        self.assertEqual(self.clock.sleep.call_count, 2)

    def test_returns_verified_snapshot_without_rereading_marker(self):
        self.publish(self.runtime)
        def probe(_number):
            self.marker.unlink()
            return True
        self.canary.side_effect = probe
        self.assertEqual(self.wait(), self.runtime)
        self.clock.sleep.assert_not_called()

    def test_missing_partial_and_stale_markers_fail_at_bounded_deadline(self):
        for marker in (None, "{", {}, {**self.runtime, "pid": 1},
                       {**self.runtime, "port": self.port - 1}):
            with self.subTest(marker=marker):
                self.publish(marker)
                self.now = 0
                self.clock.sleep.reset_mock()
                with self.assertRaisesRegex(AssertionError, "readiness deadline: fixture diagnostic"):
                    self.wait()
                self.assertEqual(self.now, 30)
                self.assertLessEqual(self.clock.sleep.call_count, 301)
        self.canary.assert_not_called()

    def test_dead_child_fails_immediately_with_log(self):
        self.publish(self.runtime)
        self.child.poll.return_value = 7
        self.child.returncode = 7
        with self.assertRaisesRegex(AssertionError, "core exited 7: fixture diagnostic"):
            self.wait()
        self.canary.assert_not_called()
        self.clock.sleep.assert_not_called()

    def test_child_exit_during_successful_probe_is_not_ready(self):
        self.publish(self.runtime)
        self.child.poll.side_effect = [None, 9]
        self.child.returncode = 9
        with self.assertRaisesRegex(AssertionError, "core exited 9: fixture diagnostic"):
            self.wait()
        self.canary.assert_called_once_with(self.port)
        self.clock.sleep.assert_not_called()

    def test_child_exit_during_failed_probe_at_deadline_preserves_exit_diagnostic(self):
        self.publish(self.runtime)
        for raises in (False, True):
            with self.subTest(raises=raises):
                self.now = 0
                self.child.poll.side_effect = [None, 9]
                self.child.returncode = 9
                def probe(_number):
                    self.now = 30
                    if raises:
                        raise OSError("connection closed")
                    return False
                self.canary.side_effect = probe
                with self.assertRaisesRegex(AssertionError, "core exited 9: fixture diagnostic"):
                    self.wait()
        self.clock.sleep.assert_not_called()

    def test_successful_probe_at_or_after_deadline_is_not_ready(self):
        self.publish(self.runtime)
        for finished in (30, 31):
            with self.subTest(finished=finished):
                self.now = 0
                def probe(_number):
                    self.now = finished
                    return True
                self.canary.side_effect = probe
                with self.assertRaisesRegex(AssertionError, "readiness deadline: fixture diagnostic"):
                    self.wait()
        self.clock.sleep.assert_not_called()


class HandoffPlatformGuardTests(unittest.TestCase):
    def test_import_cannot_launch_proxy_or_allocate_fixture_on_any_platform(self):
        for platform in ("linux", "darwin", "win32"):
            with (
                self.subTest(platform=platform),
                mock.patch("sys.platform", platform),
                mock.patch("subprocess.Popen") as popen,
                mock.patch("tempfile.TemporaryDirectory") as temporary,
            ):
                load_fixture()
                popen.assert_not_called()
                temporary.assert_not_called()

    def test_main_skips_unsupported_platform_before_any_fixture_work(self):
        fixture = load_fixture()
        for platform in ("darwin", "win32", "freebsd"):
            with (
                self.subTest(platform=platform),
                mock.patch.object(fixture.sys, "platform", platform),
                mock.patch.object(fixture.subprocess, "Popen") as popen,
                mock.patch.object(fixture.tempfile, "TemporaryDirectory") as temporary,
                contextlib.redirect_stdout(io.StringIO()) as output,
            ):
                fixture.main()
                popen.assert_not_called()
                temporary.assert_not_called()
                self.assertIn("SKIP: use isolated Linux", output.getvalue())


if __name__ == "__main__":
    unittest.main()
