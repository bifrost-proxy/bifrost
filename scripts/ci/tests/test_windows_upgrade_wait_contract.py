"""Exercise the Windows upgrade source guard without processes or host changes."""
from __future__ import annotations

import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[3]
SHELL = ROOT / "e2e-tests/tests/test_upgrade_restart_e2e.sh"
UPGRADE = Path("crates/bifrost-cli/src/commands/upgrade.rs")
RESTART = Path("crates/bifrost-cli/src/commands/upgrade/restart.rs")
WAIT = Path("crates/bifrost-cli/src/commands/upgrade/windows_parent_wait.ps1")
BASH = shutil.which("bash")


@unittest.skipUnless(BASH, "bash is required")
class WindowsUpgradeWaitContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        match = re.search(
            r"^test_windows_upgrade_defers_self_replacement_in_source\(\) \{\n.*?^\}",
            SHELL.read_text(), re.MULTILINE | re.DOTALL,
        )
        if match is None:
            raise AssertionError("Windows upgrade shell guard was not found")
        cls.guard = match.group(0)

    def run_guard(self, changes=None):
        with tempfile.TemporaryDirectory(prefix="bifrost-upgrade-contract-") as tmp:
            root = Path(tmp)
            for path in (UPGRADE, RESTART, WAIT):
                content = (ROOT / path).read_text()
                for old, new in (changes or {}).get(path, []):
                    self.assertIn(old, content, f"mutation must apply to {path}")
                    content = content.replace(old, new)
                target = root / path
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text(content)
            # Execute the exact guard only. Never source the E2E main, start a
            # daemon, run upgrade, or install its cleanup/signal handlers.
            script = "\n".join((
                "set -uo pipefail", 'PROJECT_DIR="$1"',
                "_log_info() { :; }", "_log_pass() { echo PASS; }",
                "_log_fail() { echo FAIL; }", self.guard,
                "test_windows_upgrade_defers_self_replacement_in_source",
            ))
            return subprocess.run(
                [BASH, "-c", script, "windows-upgrade-contract", os.fspath(root)],
                capture_output=True, text=True, timeout=10,
            )

    def test_current_shared_helper_satisfies_guard(self):
        result = self.run_guard()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("PASS", result.stdout)

    def test_broken_parent_wait_contracts_are_rejected(self):
        mutations = [
            (RESTART, 'include_str!("windows_parent_wait.ps1")', '""'),
            (RESTART, 'Wait-UpgradeParentExit $ParentPid 120000',
             'Wait-UpgradeParentExit $ParentPid 12000'),
            (WAIT, '$parent = Get-Process -Id $ParentPid', '$parent = $null #'),
            (WAIT, 'if (-not $parent.WaitForExit($TimeoutMilliseconds))',
             'if ($parent.WaitForExit($TimeoutMilliseconds))'),
            (WAIT, 'throw "parent process $ParentPid did not exit before timeout"', 'return'),
            (WAIT, '} finally {', '} catch {'),
            (WAIT, '$parent.Dispose()', '# disposal removed'),
            (WAIT, '$parent.Dispose()',
             '$parent.Dispose()\n  if (Get-Process -Id $ParentPid) { throw "still present" }'),
        ]
        for path, old, new in mutations:
            with self.subTest(contract=old, replacement=new):
                result = self.run_guard({path: [(old, new)]})
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("FAIL", result.stdout)

    def test_existing_staging_and_restart_contracts_are_still_required(self):
        mutations = [
            (UPGRADE, "DeferredWindows", "UnstagedWindows"),
            (UPGRADE, "unique_pending_binary_path", "direct_binary_path"),
            (RESTART, "schedule_windows_deferred_install", "install_immediately"),
            (RESTART, 'Move-Item -LiteralPath $PendingPath -Destination $TargetPath -Force',
             'Copy-Item -LiteralPath $PendingPath -Destination $TargetPath -Force'),
            (RESTART, 'Start-Process -FilePath $TargetPath -ArgumentList $restartArgs',
             'Get-Item -LiteralPath $TargetPath'),
            (RESTART, "Proxy restart scheduled with the new version", "restart skipped"),
        ]
        for path, old, new in mutations:
            with self.subTest(contract=old):
                result = self.run_guard({path: [(old, new)]})
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("FAIL", result.stdout)


if __name__ == "__main__":
    unittest.main()
