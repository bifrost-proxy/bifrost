"""Exercise CI's FFmpeg setup with no host package manager or network access."""
from __future__ import annotations

import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import textwrap
import unittest


ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = ROOT / ".github/workflows/ci.yml"
BASH = shutil.which("bash")
JOBS = ("coverage", "e2e-shell")
STEP = "Install FFmpeg for ASR source compression E2E"
ACQUIRE_OPTIONS = [
    "-o", "Acquire::Retries=3",
    "-o", "Acquire::http::Timeout=30",
    "-o", "Acquire::https::Timeout=30",
]
UPDATE = ["sudo", "apt-get", *ACQUIRE_OPTIONS, "update"]
INSTALL = ["sudo", "apt-get", *ACQUIRE_OPTIONS,
           "install", "--yes", "--no-install-recommends", "ffmpeg"]
VERSION = ["ffmpeg", "-version"]

# Both executables are fakes. The fake sudo records arguments but never invokes
# apt-get; a successful install only creates a fake ffmpeg in the temporary PATH.
FAKE = r'''
import json, os, sys
from pathlib import Path
command = Path(sys.argv[0]).name
args = sys.argv[1:]
with Path(os.environ["TEST_CALLS"]).open("a") as calls:
    calls.write(json.dumps([command, *args]) + "\n")
if command == "ffmpeg":
    assert args == ["-version"], args
    print("fake ffmpeg version")
    sys.exit(int(os.environ["TEST_VERSION_STATUS"]))
assert command == "sudo" and args[0] == "apt-get", (command, args)
if "update" in args:
    sys.exit(int(os.environ["TEST_UPDATE_STATUS"]))
assert "install" in args, args
status = int(os.environ["TEST_INSTALL_STATUS"])
if status == 0 and os.environ["TEST_CREATE_FFMPEG"] == "1":
    binary = Path(sys.argv[0]).with_name("ffmpeg")
    binary.write_text(Path(sys.argv[0]).read_text())
    binary.chmod(0o700)
sys.exit(status)
'''


def ffmpeg_step(job: str) -> tuple[str, str]:
    """Read the actual inline step without adding a YAML dependency to CI."""
    workflow = WORKFLOW.read_text()
    job_match = re.search(rf"^  {re.escape(job)}:\n(.*?)(?=^  \S|\Z)",
                          workflow, re.MULTILINE | re.DOTALL)
    if job_match is None:
        raise AssertionError(f"Missing job: {job}")
    steps = re.findall(rf"^      - name: {re.escape(STEP)}\n(.*?)(?=^      - |\Z)",
                       job_match[1], re.MULTILINE | re.DOTALL)
    if len(steps) != 1:
        raise AssertionError(f"Expected exactly one FFmpeg setup in {job}")
    header, script = steps[0].split("        run: |\n", 1)
    return header, textwrap.dedent(script)


@unittest.skipUnless(BASH, "bash is required")
class FFmpegSetupTests(unittest.TestCase):
    def run_setup(self, job: str, *, present: bool = False, update_status: int = 0,
                  install_status: int = 0, version_status: int = 0,
                  create_ffmpeg: bool = True) -> tuple[subprocess.CompletedProcess, list]:
        with tempfile.TemporaryDirectory(prefix="bifrost-ffmpeg-setup-") as tmp:
            directory = Path(tmp)
            calls = directory / "calls.jsonl"
            for name in (["sudo", "ffmpeg"] if present else ["sudo"]):
                binary = directory / name
                binary.write_text(f"#!{sys.executable}\n{FAKE}")
                binary.chmod(0o700)
            # No inherited PATH, shell startup hooks, or exported shell functions:
            # neither a real sudo/apt-get nor a host ffmpeg can enter these tests.
            env = {
                "PATH": tmp,
                "TEST_CALLS": str(calls),
                "TEST_UPDATE_STATUS": str(update_status),
                "TEST_INSTALL_STATUS": str(install_status),
                "TEST_VERSION_STATUS": str(version_status),
                "TEST_CREATE_FFMPEG": "1" if create_ffmpeg else "0",
            }
            _, script = ffmpeg_step(job)
            result = subprocess.run(
                [BASH, "--noprofile", "--norc", "-e", "-c", script],
                cwd=directory, env=env, text=True, capture_output=True, timeout=10,
            )
            recorded = [json.loads(line) for line in calls.read_text().splitlines()] \
                if calls.exists() else []
            return result, recorded

    def test_both_steps_have_a_fifteen_minute_bound_and_no_failure_override(self) -> None:
        for job in JOBS:
            with self.subTest(job=job):
                header, _ = ffmpeg_step(job)
                self.assertRegex(header, r"(?m)^        timeout-minutes: 15$")
                self.assertNotIn("continue-on-error", header)

    def test_preinstalled_ffmpeg_is_verified_without_package_manager_calls(self) -> None:
        for job in JOBS:
            with self.subTest(job=job):
                result, calls = self.run_setup(job, present=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual(calls, [VERSION])

    def test_missing_ffmpeg_is_installed_with_bounded_acquisition_then_verified(self) -> None:
        for job in JOBS:
            with self.subTest(job=job):
                result, calls = self.run_setup(job)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual(calls, [UPDATE, INSTALL, VERSION])

    def test_update_and_install_failures_stop_without_fallback(self) -> None:
        for job in JOBS:
            for options, expected in (
                ({"update_status": 100}, [UPDATE]),
                ({"install_status": 100}, [UPDATE, INSTALL]),
            ):
                with self.subTest(job=job, options=options):
                    result, calls = self.run_setup(job, **options)
                    self.assertEqual(result.returncode, 100, result.stdout + result.stderr)
                    self.assertEqual(calls, expected)

    def test_successful_install_without_an_executable_still_fails(self) -> None:
        for job in JOBS:
            with self.subTest(job=job):
                result, calls = self.run_setup(job, create_ffmpeg=False)
                self.assertEqual(result.returncode, 127, result.stdout + result.stderr)
                self.assertEqual(calls, [UPDATE, INSTALL])
                self.assertIn("ffmpeg: command not found", result.stderr)

    def test_broken_ffmpeg_is_fatal_before_or_after_installation(self) -> None:
        for job in JOBS:
            for present in (False, True):
                with self.subTest(job=job, present=present):
                    result, calls = self.run_setup(job, present=present, version_status=23)
                    self.assertEqual(result.returncode, 23, result.stdout + result.stderr)
                    self.assertEqual(calls, [VERSION] if present else [UPDATE, INSTALL, VERSION])


if __name__ == "__main__":
    unittest.main()
