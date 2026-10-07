"""Native recovery acceptance assertions tested without Bifrost or OS commands."""
from __future__ import annotations

import copy
import contextlib
import io
import json
import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "e2e-tests/tests/test_system_proxy_reconcile_stability.sh"
SPEC = importlib.util.spec_from_file_location("native_acceptance", ROOT / "e2e-tests/test_utils/macos_proxy_acceptance.py")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
BASH = shutil.which("bash")


def protocol(server="corp.example", port=8080, enabled=False):
    return {"enabled": enabled, "server": server, "port": port, "authenticated": False}


def baseline():
    return {"Wi-Fi": {"http": protocol(), "https": protocol("secure.example", 8443),
                      "bypass": ["localhost", "*.corp.example"],
                      "untouched": {"socks": "Enabled: No\nServer: socks.example\nPort: 1080\n",
                                    "pac": "URL: https://corp.example/proxy.pac\nEnabled: No\n",
                                    "autodiscovery": "Auto Proxy Discovery: Off\n"}}}


class NativeReadbackTests(unittest.TestCase):
    def test_disabled_endpoint_is_not_erased(self):
        self.assertEqual(MODULE.parse_protocol("Enabled: No\nServer: corp.example\nPort: 8080\nAuthenticated Proxy Enabled: 0\n"), protocol())

    def test_empty_endpoint_and_zero_port_remain_distinct(self):
        self.assertEqual(MODULE.parse_protocol("Enabled: No\nServer:\nPort: 0\nAuthenticated Proxy Enabled: 0\n"), protocol("", 0))

    def test_bad_readbacks_fail_closed(self):
        valid = "Enabled: No\nServer: corp.example\nPort: 8080\nAuthenticated Proxy Enabled: 0\n"
        for invalid in (valid.replace("Enabled: No", "Enabled: Maybe"), valid.replace("Port: 8080", "Port: -1"),
                        valid.replace("Port: 8080", "Port: 70000"), valid.replace("Server: corp.example\n", ""),
                        valid + "Enabled: Yes\n", valid + "** Error: permission denied\n", "** Error: permission denied"):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                MODULE.parse_protocol(invalid)

    def test_bypass_preserves_values_and_rejects_empty_error(self):
        self.assertEqual(MODULE.parse_bypass("localhost\n*.corp.example\n"), ["localhost", "*.corp.example"])
        self.assertEqual(MODULE.parse_bypass("There aren't any bypass domains set on Wi-Fi.\n"), [])
        for invalid in ("", "** Error: permission denied"):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                MODULE.parse_bypass(invalid)

    def test_zero_exit_with_native_error_is_rejected_without_running_os_tool(self):
        with patch.object(MODULE.subprocess, "run", return_value=SimpleNamespace(
                stdout="Enabled: No\n", stderr="** Error: permission denied")) as run:
            with self.assertRaises(ValueError):
                MODULE.native_read("-getwebproxy", "Wi-Fi")
            self.assertEqual(run.call_args.args[0], ["/usr/sbin/networksetup", "-getwebproxy", "Wi-Fi"])
            self.assertEqual(run.call_args.kwargs["env"]["LC_ALL"], "C")

    def test_capture_only_uses_read_queries(self):
        calls = []
        def recorded(*args):
            calls.append(args)
            if args == ("-listallnetworkservices",):
                return "An asterisk (*) denotes that a network service is disabled.\nWi-Fi\n*Disabled\n"
            if args[0] == "-getproxybypassdomains":
                return "localhost\n"
            return "Enabled: No\nServer:\nPort: 0\nAuthenticated Proxy Enabled: 0\n"
        self.assertEqual(list(MODULE.capture(recorded)), ["Wi-Fi"])
        self.assertEqual(calls, [("-listallnetworkservices",), ("-getwebproxy", "Wi-Fi"),
                                 ("-getsecurewebproxy", "Wi-Fi"), ("-getproxybypassdomains", "Wi-Fi"),
                                 ("-getsocksfirewallproxy", "Wi-Fi"), ("-getautoproxyurl", "Wi-Fi"),
                                 ("-getproxyautodiscovery", "Wi-Fi")])

    def test_unmanaged_configuration_must_stay_byte_identical_in_all_states(self):
        original = baseline()
        active = copy.deepcopy(original)
        for field in ("http", "https"):
            active["Wi-Fi"][field] = protocol("127.0.0.1", 18889, True)
        for field in ("socks", "pac", "autodiscovery"):
            for current, assertion in ((original, lambda value: MODULE.assert_disabled(original, value)),
                                       (active, lambda value: MODULE.assert_active(original, value, 18889))):
                changed = copy.deepcopy(current)
                changed["Wi-Fi"]["untouched"][field] += "changed\n"
                with self.subTest(field=field), self.assertRaisesRegex(AssertionError, "out-of-scope"):
                    assertion(changed)
        with self.assertRaises(ValueError):
            MODULE.opaque_readback("")

    def test_disabled_checks_each_protocol_and_present_endpoint(self):
        original = baseline()
        self.assertIsNone(MODULE.assert_disabled(original, copy.deepcopy(original)))
        for field in ("http", "https"):
            for key, value in (("enabled", True), ("server", "127.0.0.1"), ("port", 18889), ("authenticated", True)):
                actual = copy.deepcopy(original)
                actual["Wi-Fi"][field][key] = value
                with self.subTest(field=field, key=key), self.assertRaises(AssertionError):
                    MODULE.assert_disabled(original, actual)

    def test_bypass_and_service_changes_cannot_be_hidden(self):
        original = baseline()
        actual = copy.deepcopy(original)
        actual["Wi-Fi"]["bypass"] = ["localhost"]
        with self.assertRaisesRegex(AssertionError, "bypass"):
            MODULE.assert_disabled(original, actual)
        with self.assertRaisesRegex(AssertionError, "service set"):
            MODULE.assert_disabled(original, {})

    def test_empty_dormant_residual_is_reported_not_called_exact_restore(self):
        original = baseline()
        original["Wi-Fi"]["http"] = protocol("", 0)
        actual = copy.deepcopy(original)
        actual["Wi-Fi"]["http"] = protocol("127.0.0.1", 18889)
        with self.assertRaisesRegex(AssertionError, "INCOMPLETE RESTORATION"):
            MODULE.assert_disabled(original, actual)
        actual["Wi-Fi"]["http"]["enabled"] = True
        with self.assertRaisesRegex(AssertionError, "routing enabled"):
            MODULE.assert_disabled(original, actual)

    def test_active_requires_both_real_targets(self):
        original = baseline()
        actual = copy.deepcopy(original)
        for field in ("http", "https"):
            actual["Wi-Fi"][field] = protocol("127.0.0.1", 18889, True)
        MODULE.assert_active(original, actual, 18889)
        for field in ("http", "https"):
            broken = copy.deepcopy(actual)
            broken["Wi-Fi"][field]["enabled"] = False
            with self.assertRaises(AssertionError):
                MODULE.assert_active(original, broken, 18889)
        with self.assertRaises(AssertionError):
            MODULE.assert_active(original, actual, 18890)

    def test_nonroot_allowance_is_only_empty_metadata_with_routing_off(self):
        original = baseline()
        original["Wi-Fi"]["http"] = protocol("", 0)
        actual = copy.deepcopy(original)
        actual["Wi-Fi"]["http"] = protocol("127.0.0.1", 18889)
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            MODULE.assert_disabled(original, actual, allow_empty_metadata=True)
        self.assertIn("NONROOT LIMITATION", output.getvalue())
        with self.assertRaises(AssertionError):
            MODULE.assert_disabled(original, actual)
        for mutate in (lambda value: value["Wi-Fi"]["http"].update(enabled=True),
                       lambda value: value["Wi-Fi"]["https"].update(server="wrong.example"),
                       lambda value: value["Wi-Fi"].update(bypass=[]),
                       lambda value: value["Wi-Fi"]["untouched"].update(pac="changed")):
            broken = copy.deepcopy(actual)
            mutate(broken)
            with contextlib.redirect_stdout(io.StringIO()), self.assertRaises(AssertionError):
                MODULE.assert_disabled(original, broken, allow_empty_metadata=True)

    def test_same_lease_requires_original_generation_baseline_and_phase(self):
        with tempfile.TemporaryDirectory() as tmp:
            path, before = Path(tmp) / "journal.json", Path(tmp) / "before.json"
            state = {"generation": "fixture-generation", "phase": "applied", "applied": True,
                     "macos_services": [{"name": "Wi-Fi", "fields": [{"field": "http", "before": {"host": ""}}]}]}
            path.write_text(json.dumps(state))
            before.write_text(json.dumps(MODULE.lease_identity(path)))
            state.update(phase="suspended", applied=False)
            path.write_text(json.dumps(state))
            MODULE.check_lease(path, before, "suspended")
            for changed in ({**state, "generation": "new-generation"}, {**state, "applied": True},
                            {**state, "macos_services": []}):
                path.write_text(json.dumps(changed))
                with self.assertRaises(AssertionError):
                    MODULE.check_lease(path, before, "suspended")

    def test_stopped_state_requires_successful_real_state_readback(self):
        for state, expected in (("T", True), ("T+", True), ("S", False), ("R", False)):
            with patch.object(MODULE.subprocess, "run", return_value=SimpleNamespace(returncode=0, stdout=state, stderr="")):
                self.assertEqual(MODULE.process_stopped(424242), expected)
        with patch.object(MODULE.subprocess, "run", return_value=SimpleNamespace(returncode=1, stdout="", stderr="")), self.assertRaises(AssertionError):
            MODULE.process_stopped(424242)

    def test_process_identity_distinguishes_absence_from_inspection_failure(self):
        with patch.object(MODULE.subprocess, "run", return_value=SimpleNamespace(returncode=1, stdout="", stderr="")):
            self.assertEqual(MODULE.process_identity(424242), "")
        for result in (SimpleNamespace(returncode=2, stdout="", stderr="ps failed"),
                       SimpleNamespace(returncode=1, stdout="", stderr="permission denied"),
                       SimpleNamespace(returncode=0, stdout="", stderr="")):
            with self.subTest(result=result), patch.object(MODULE.subprocess, "run", return_value=result), self.assertRaises(AssertionError):
                MODULE.process_identity(424242)

    @unittest.skipUnless(os.name == "posix", "temporary flock fixture requires POSIX")
    def test_absent_core_never_authorizes_a_signal_or_privileged_cleanup(self):
        with tempfile.TemporaryDirectory() as tmp:
            data = Path(tmp)
            (data / ".system_proxy.lock").touch()
            expected = f"{os.getppid()} Mon Oct 7 01:02:03 2026"
            signals = []
            with patch("fcntl.flock") as lock, self.assertRaisesRegex(AssertionError, "absent"):
                MODULE.control_owned_core(data, 424242, expected, "retire", read_identity=lambda _: "", send_signal=lambda *args: signals.append(args))
            lock.assert_not_called()
            self.assertEqual(signals, [])

    @unittest.skipUnless(os.name == "posix", "temporary flock fixture requires POSIX")
    def test_retirement_holds_real_temporary_lock_and_only_signals_matching_child(self):
        import fcntl
        with tempfile.TemporaryDirectory() as tmp:
            data = Path(tmp)
            lock = data / ".system_proxy.lock"
            lock.touch()
            journal = data / "proxy_state.json"
            journal.write_text("preserved fixture journal")
            expected = f"{os.getppid()} Mon Oct 7 01:02:03 2026"
            observations = iter([expected, expected, ""])
            signals = []
            def send(pid, signal):
                with lock.open("r+") as contender, self.assertRaises(BlockingIOError):
                    fcntl.flock(contender, fcntl.LOCK_EX | fcntl.LOCK_NB)
                signals.append((pid, signal))
            MODULE.control_owned_core(data, 424242, expected, "retire", read_identity=lambda _: next(observations), send_signal=send)
            self.assertEqual(signals, [(424242, MODULE.signal.SIGKILL)])
            self.assertEqual(journal.read_text(), "preserved fixture journal")
            with lock.open("r+") as contender:
                fcntl.flock(contender, fcntl.LOCK_EX | fcntl.LOCK_NB)
            for observations in (["different child"], [expected, "different child"], [expected, ""]):
                sequence = iter(observations)
                signals.clear()
                with self.assertRaises(AssertionError):
                    MODULE.control_owned_core(data, 424242, expected, "retire", read_identity=lambda _: next(sequence), send_signal=send)
                self.assertEqual(signals, [])

    @unittest.skipUnless(os.name == "posix", "temporary flock fixture requires POSIX")
    def test_pause_is_lock_held_and_a_busy_writer_prevents_signalling(self):
        import fcntl
        with tempfile.TemporaryDirectory() as tmp:
            data = Path(tmp)
            lock_path = data / ".system_proxy.lock"
            lock_path.touch()
            expected = f"{os.getppid()} Mon Oct 7 01:02:03 2026"
            signals = []
            def send(pid, signal):
                with lock_path.open("r+") as contender, self.assertRaises(BlockingIOError):
                    fcntl.flock(contender, fcntl.LOCK_EX | fcntl.LOCK_NB)
                signals.append((pid, signal))
            stopped = iter([False, True])
            def read_stopped(_):
                with lock_path.open("r+") as contender, self.assertRaises(BlockingIOError):
                    fcntl.flock(contender, fcntl.LOCK_EX | fcntl.LOCK_NB)
                return next(stopped)
            MODULE.control_owned_core(data, 424242, expected, "pause", read_identity=lambda _: expected, send_signal=send, read_stopped=read_stopped)
            self.assertEqual(signals, [(424242, MODULE.signal.SIGSTOP)])
            signals.clear()
            with patch("fcntl.flock", side_effect=BlockingIOError), patch.object(MODULE.time, "monotonic", side_effect=[0, 181]), self.assertRaisesRegex(AssertionError, "native writer"):
                MODULE.control_owned_core(data, 424242, expected, "pause", read_identity=lambda _: expected, send_signal=send)
            self.assertEqual(signals, [])

    def test_intent_check_requires_enabled_and_revision(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "config.toml"
            text = '[system_proxy]\nenabled=true\nintent_revision=7\nrecovery_mode="fail_open"\nrecovery_grace_secs=3\n'
            path.write_text(text)
            self.assertEqual(MODULE.intent(path)["intent_revision"], 7)
            path.write_text(text.replace("enabled=true", "enabled=false"))
            with self.assertRaisesRegex(AssertionError, "intent was lost"):
                MODULE.intent(path)


@unittest.skipUnless(BASH, "bash is required")
class NativeFixtureContractTests(unittest.TestCase):
    def gate(self, env):
        text = SCRIPT.read_text()
        function = "require_disposable_macos_ci() {" + text.split("require_disposable_macos_ci() {", 1)[1].split("\n}", 1)[0] + "\n}"
        return subprocess.run([BASH, "-c", function + "\nrequire_disposable_macos_ci"],
                              env={"PATH": os.environ["PATH"], **env}, capture_output=True, text=True, timeout=5)

    def test_personal_mac_and_self_hosted_are_rejected(self):
        approved = {"GITHUB_ACTIONS": "true", "RUNNER_ENVIRONMENT": "github-hosted", "RUNNER_OS": "macOS",
                    "GITHUB_REPOSITORY": "bifrost-proxy/bifrost", "GITHUB_RUN_ID": "123", "BIFROST_NATIVE_PROXY_CI": "1"}
        self.assertEqual(self.gate(approved).returncode, 0)
        self.assertNotEqual(self.gate({}).returncode, 0)
        for key in approved:
            env = dict(approved)
            env.pop(key)
            with self.subTest(missing=key):
                self.assertNotEqual(self.gate(env).returncode, 0)
        self.assertNotEqual(self.gate({**approved, "RUNNER_ENVIRONMENT": "self-hosted"}).returncode, 0)

    def elevation(self, uid="501", sudo_status=0, override=None):
        text = SCRIPT.read_text()
        def function(name):
            return name + "() {" + text.split(name + "() {", 1)[1].split("\n}", 1)[0] + "\n}"
        approved = {"GITHUB_ACTIONS": "true", "RUNNER_ENVIRONMENT": "github-hosted", "RUNNER_OS": "macOS",
                    "GITHUB_REPOSITORY": "bifrost-proxy/bifrost", "GITHUB_RUN_ID": "123", "BIFROST_NATIVE_PROXY_CI": "1",
                    "BIFROST_BIN": "/workspace/release artifact/bifrost", "BIFROST_NATIVE_PYTHON": "/opt/selected/python3",
                    "PROXY_PORT": "18889", "BIFROST_E2E_REPORT_DIR": "/workspace/reports",
                    "SCRIPT_PATH": str(SCRIPT), "MOCK_UID": uid, "MOCK_SUDO_STATUS": str(sudo_status),
                    "UNRELATED_CREDENTIAL": "do-not-forward", "HOME": "/personal/home",
                    "BIFROST_DATA_DIR": "/personal/data", "HTTPS_PROXY": "do-not-forward"}
        approved.update(override or {})
        script = 'fixture_effective_uid() { printf "%s\\n" "$MOCK_UID"; };\n'
        script += 'exec() { printf "%s\\0" "$@"; return "$MOCK_SUDO_STATUS"; };\n'
        result = subprocess.run([BASH, "-c", script + function("require_disposable_macos_ci")
                                 + "\n" + function("elevate_native_fixture") + "\nelevate_native_fixture"],
                                env={"PATH": os.environ["PATH"], **approved}, capture_output=True, timeout=5)
        return result, [part.decode() for part in result.stdout.split(b"\0") if part]

    def test_elevation_targets_only_this_script_with_clean_allowlisted_environment(self):
        result, argv = self.elevation()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(argv[:5], ["/usr/bin/sudo", "-n", "--", "/usr/bin/env", "-i"])
        self.assertEqual(argv[-2:], ["/bin/bash", str(SCRIPT)])
        assignments = dict(entry.split("=", 1) for entry in argv[5:-2])
        self.assertEqual(set(assignments), {"PATH", "LC_ALL", "PYTHONNOUSERSITE", "PYTHONDONTWRITEBYTECODE",
            "GITHUB_ACTIONS", "RUNNER_ENVIRONMENT", "RUNNER_OS", "GITHUB_REPOSITORY", "GITHUB_RUN_ID",
            "BIFROST_NATIVE_PROXY_CI", "BIFROST_NATIVE_PROXY_ELEVATED", "BIFROST_BIN", "BIFROST_NATIVE_PYTHON",
            "PROXY_PORT", "BIFROST_E2E_REPORT_DIR"})
        self.assertEqual(assignments["BIFROST_BIN"], "/workspace/release artifact/bifrost")
        self.assertEqual(assignments["BIFROST_NATIVE_PYTHON"], "/opt/selected/python3")
        self.assertEqual(assignments["PATH"], "/usr/bin:/bin:/usr/sbin:/sbin")
        self.assertNotIn("run_all_e2e.sh", " ".join(argv))
        self.assertNotIn("do-not-forward", " ".join(argv))
        self.assertNotIn("/personal", " ".join(argv))

    def test_elevation_fails_closed_without_approval_or_noninteractive_privilege(self):
        result, argv = self.elevation(sudo_status=1)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("-n", argv)
        for override in ({"BIFROST_NATIVE_PROXY_CI": "0"}, {"RUNNER_ENVIRONMENT": "self-hosted"},
                         {"BIFROST_NATIVE_PROXY_ELEVATED": "1"}):
            with self.subTest(override=override):
                result, argv = self.elevation(override=override)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(argv, [])
        result, argv = self.elevation(uid="0")
        self.assertEqual(result.returncode, 0)
        self.assertEqual(argv[:2], ["/usr/bin/env", "-i"])
        self.assertEqual(argv[-2:], ["/bin/bash", str(SCRIPT)])
        result, argv = self.elevation(uid="0", override={"BIFROST_NATIVE_PROXY_ELEVATED": "1"})
        self.assertEqual(result.returncode, 0)
        self.assertEqual(argv, [])

    def test_nonroot_mode_never_elevates_product_and_rejects_root(self):
        result, argv = self.elevation(override={"BIFROST_NATIVE_PROXY_MODE": "nonroot-recovery"})
        self.assertEqual(result.returncode, 0)
        self.assertEqual(argv, [])
        for uid, override in (("0", {}), ("501", {"BIFROST_NATIVE_PROXY_ELEVATED": "1"})):
            result, argv = self.elevation(uid=uid, override={"BIFROST_NATIVE_PROXY_MODE": "nonroot-recovery", **override})
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(argv, [])

    def test_privileged_companion_cleanup_is_only_selected_binary_and_temp_data(self):
        text = SCRIPT.read_text()
        function = "privileged_fixture_cleanup_command() {" + text.split("privileged_fixture_cleanup_command() {", 1)[1].split("\n}", 1)[0] + "\n}"
        script = 'require_disposable_macos_ci() { return "$GUARD_STATUS"; };\ncommand() { printf "%s\\0" "$@"; return "$SUDO_STATUS"; };\n'
        env = {"PATH": os.environ["PATH"], "HOME": "/tmp/fixture/home", "XDG_CONFIG_HOME": "/tmp/fixture/home/config",
               "XDG_DATA_HOME": "/tmp/fixture/home/data", "BIFROST_DATA_DIR": "/tmp/fixture/data",
               "BIFROST_BIN": "/workspace/selected/bifrost", "UNRELATED_CREDENTIAL": "never-forward",
               "GUARD_STATUS": "0", "SUDO_STATUS": "0"}
        result = subprocess.run([BASH, "-c", script + function + "\nprivileged_fixture_cleanup_command"],
                                env=env, capture_output=True, timeout=5)
        self.assertEqual(result.returncode, 0)
        argv = [part.decode() for part in result.stdout.split(b"\0") if part]
        self.assertEqual(argv[:5], ["/usr/bin/sudo", "-n", "--", "/usr/bin/env", "-i"])
        self.assertEqual(argv[-7:], ["/workspace/selected/bifrost", "--log-output", "console,file", "system-proxy", "cleanup", "--data-dir", "/tmp/fixture/data"])
        self.assertNotIn("never-forward", " ".join(argv))
        self.assertNotIn("/bin/bash", argv)
        for key in ("GUARD_STATUS", "SUDO_STATUS"):
            result = subprocess.run([BASH, "-c", script + function + "\nprivileged_fixture_cleanup_command"],
                                    env={**env, key: "1"}, capture_output=True, timeout=5)
            self.assertNotEqual(result.returncode, 0)

    def test_companion_serialization_and_loopback_binding(self):
        wrapper = (ROOT / "e2e-tests/tests/test_system_proxy_nonroot_recovery.sh").read_text()
        self.assertIn("BIFROST_NATIVE_PROXY_MODE=nonroot-recovery", wrapper)
        self.assertNotIn("sudo", wrapper.split("export BIFROST_NATIVE_PROXY_MODE", 1)[1])
        text = SCRIPT.read_text()
        self.assertIn('"$BIFROST_BIN" --host 127.0.0.1 --port "$PROXY_PORT" start', text)
        isolated = (ROOT / "scripts/run_all_e2e.sh").read_text().split("ISOLATED_AFTER_TESTS=(", 1)[1].split(")", 1)[0]
        self.assertIn('"test_system_proxy_nonroot_recovery.sh"', isolated)
        cleanup = text.split("cleanup_nonroot_fixture() {", 1)[1].split("\n}", 1)[0]
        self.assertLess(cleanup.index("retire-core"), cleanup.index("privileged_fixture_cleanup_command"))
        self.assertNotIn(" TERM", cleanup)
        self.assertIn("check-lease", text)

    def test_port_guard_rejects_production_octal_and_overflow_values(self):
        text = SCRIPT.read_text()
        function = "require_native_test_port() {" + text.split("require_native_test_port() {", 1)[1].split("\n}", 1)[0] + "\n}"
        for port in ("9900", "09900", "0009900", "999999999999999999999", "0", "1023", "65536", "-1", "18889;echo unsafe"):
            with self.subTest(port=port):
                result = subprocess.run([BASH, "-c", function + "\nrequire_native_test_port"],
                                        env={"PATH": os.environ["PATH"], "PROXY_PORT": port}, capture_output=True, timeout=5)
                self.assertNotEqual(result.returncode, 0)
        for port in ("1024", "18889", "65535"):
            with self.subTest(port=port):
                result = subprocess.run([BASH, "-c", function + "\nrequire_native_test_port"],
                                        env={"PATH": os.environ["PATH"], "PROXY_PORT": port}, capture_output=True, timeout=5)
                self.assertEqual(result.returncode, 0)

    def test_privileged_fixture_has_no_build_or_persistent_security_setup(self):
        text = SCRIPT.read_text()
        self.assertNotIn("cargo build", text)
        self.assertNotIn('source "$SCRIPT_DIR/../test_utils/process.sh"', text)
        self.assertLess(text.index("\nelevate_native_fixture\n"), text.index('TEST_ROOT="$(mktemp -d)"'))
        for required in ("export BIFROST_DISABLE_TRAY=1", "export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1",
                         "export BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1", "--skip-cert-check"):
            self.assertIn(required, text)
        for forbidden in ("ca install", "cert install", "launchd install", "sudoers", "sudo -S"):
            self.assertNotIn(forbidden, text)

    def test_final_exact_assertion_and_product_before_cleanup_are_preserved(self):
        text = SCRIPT.read_text()
        self.assertIn('if ! cmp -s "$SNAPSHOT_FILE" "$AFTER_SNAPSHOT_FILE"; then', text)
        tail = text.split("exercise_native_fail_open_resume\n", 1)[1]
        self.assertLess(tail.index('disabled "$NATIVE_SNAPSHOT"'), tail.index("restore_proxy_snapshot"))
        self.assertLess(tail.index('check-intent "$BIFROST_DATA_DIR/config.toml"'), tail.index("restore_proxy_snapshot"))
        self.assertIn("trap cleanup EXIT", text)
        self.assertIn('signal_owned_child "$PROXY_PID" "$CORE_IDENTITY" CONT', text.split("cleanup() {", 1)[1].split("\n}", 1)[0])
        for forbidden in ("-setsocksfirewallproxy", "-setautoproxyurl", "-setautoproxystate", "pkill", "killall"):
            self.assertNotIn(forbidden, text)

    def test_signal_refuses_stale_identity_without_sending_any_signal(self):
        text = SCRIPT.read_text()
        function = "signal_owned_child() {" + text.split("signal_owned_child() {", 1)[1].split("\n}", 1)[0] + "\n}"
        for current, expected, succeeds in (("123 start-a", "123 start-a", True),
                                             ("123 start-b", "123 start-a", False),
                                             ("456 start-a", "123 start-a", False),
                                             ("123 start-a", "", False)):
            with self.subTest(current=current, expected=expected):
                script = 'child_identity() { printf "%s\\n" "$CURRENT"; }; kill() { echo SIGNAL; };\n'
                result = subprocess.run([BASH, "-c", script + function + '\nsignal_owned_child 42 "$EXPECTED" TERM'],
                                        env={"PATH": os.environ["PATH"], "CURRENT": current, "EXPECTED": expected},
                                        capture_output=True, text=True, timeout=5)
                self.assertEqual(result.returncode == 0, succeeds)
                self.assertEqual("SIGNAL" in result.stdout, succeeds)

    def test_real_signal_and_same_identity_are_required(self):
        text = SCRIPT.read_text()
        self.assertIn('"$READBACK" pause-core "$BIFROST_DATA_DIR" "$PROXY_PID" "$CORE_IDENTITY"', text)
        self.assertIn('signal_owned_child "$HELPER_PID" "$HELPER_IDENTITY" TERM', text)
        self.assertIn('export BIFROST_SYSTEM_PROXY_DISABLE_LIFECYCLE_HELPER=1', text)
        self.assertIn("BIFROST_PROXY_TEST_IO_CAPABILITY_V1", text)
        cleanup = text.split("cleanup() {", 1)[1].split("\n}", 1)[0]
        self.assertNotIn('"$HELPER_IDENTITY" KILL', cleanup)
        self.assertLess(cleanup.index('wait_owned_child_exit "$HELPER_PID"'), cleanup.index('restore_proxy_snapshot'))
        self.assertIn('if [[ "$FIXTURE_MUTATED" == 1 && "$NATIVE_MODE" == direct-exact ]]', cleanup)
        self.assertIn('if heartbeat() != first:', text)
        runner = (ROOT / "scripts/run_all_e2e.sh").read_text()
        isolated = runner.split("ISOLATED_AFTER_TESTS=(", 1)[1].split(")", 1)[0]
        self.assertIn('"test_system_proxy_reconcile_stability.sh"', isolated)
        self.assertIn('helper_fail_open_applied', text)
        self.assertIn('"live core was replaced"', text)
        self.assertIn('"$successes" -ge 3', text)
        self.assertIn('"$((SECONDS - first))" -ge 2', text)
        self.assertIn('active "$NATIVE_SNAPSHOT" "$TEST_ROOT/product-resumed.json"', text)


if __name__ == "__main__":
    unittest.main()
