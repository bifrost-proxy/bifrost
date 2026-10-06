from __future__ import annotations

import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
BROWSER_TEST = ROOT / "e2e-tests/tests/test_rule_share_confirm_browser.sh"
BASH = shutil.which("bash")


@unittest.skipUnless(BASH, "bash is required")
class BrowserChromeResolverTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="bifrost-chrome-resolver-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        (self.root / "web/node_modules/@playwright/test").mkdir(parents=True)
        self.executable("uname", "printf 'Linux\\n'\n")
        source = BROWSER_TEST.read_text(encoding="utf-8")
        # Exercise the real function inside the same outer substitution as the
        # E2E, without executing any of its browser/proxy startup or cleanup.
        resolver = source.split("resolve_chrome_bin() {\n", 1)[1]
        resolver = resolver.split('\nCHROME_BIN="$(resolve_chrome_bin)"', 1)[0]
        self.script = (
            "set -euo pipefail\nresolve_chrome_bin() {\n"
            + resolver
            + '\nCHROME_BIN="$(resolve_chrome_bin)"\nprintf "%s\\n" "$CHROME_BIN"\n'
        )
        self.env = {"PATH": str(self.bin), "ROOT_DIR": str(self.root)}

    def executable(self, name: str, body: str = "exit 99\n") -> Path:
        path = self.bin / name
        path.write_text("#!/bin/sh\n" + body, encoding="utf-8")
        path.chmod(0o755)
        return path

    def resolve(self) -> str:
        result = subprocess.run(
            [BASH, "-c", self.script],
            env=self.env,
            text=True,
            capture_output=True,
            timeout=10,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stderr, "")
        return result.stdout.rstrip("\n")

    def test_explicit_override_takes_precedence(self) -> None:
        override = self.executable("explicit chrome")
        self.env["CHROME_BIN"] = str(override)
        self.executable("node", "exit 1\n")
        self.executable("chromium")
        self.assertEqual(self.resolve(), str(override))

    def test_playwright_executable_with_spaces_is_resolved(self) -> None:
        chrome = self.executable("playwright chrome")
        self.env["MOCK_CHROME_BIN"] = str(chrome)
        self.executable("node", 'printf "%s" "$MOCK_CHROME_BIN"\n')
        self.executable("chromium")
        self.assertEqual(self.resolve(), str(chrome))

    def test_failed_node_probe_falls_back_to_path(self) -> None:
        self.executable("node", "exit 1\n")
        chrome = self.executable("chromium")
        self.assertEqual(self.resolve(), str(chrome))

    def test_nonexecutable_playwright_result_falls_back_to_path(self) -> None:
        candidate = self.root / "not-executable"
        candidate.touch()
        self.env["MOCK_CHROME_BIN"] = str(candidate)
        self.executable("node", 'printf "%s" "$MOCK_CHROME_BIN"\n')
        chrome = self.executable("chromium")
        self.assertEqual(self.resolve(), str(chrome))

    def test_missing_node_falls_back_to_path(self) -> None:
        chrome = self.executable("chromium-headless-shell")
        self.assertEqual(self.resolve(), str(chrome))

    def test_missing_node_and_browser_returns_empty(self) -> None:
        self.assertEqual(self.resolve(), "")

    def test_failed_node_without_browser_returns_empty(self) -> None:
        self.executable("node", "exit 1\n")
        self.assertEqual(self.resolve(), "")


if __name__ == "__main__":
    unittest.main()
