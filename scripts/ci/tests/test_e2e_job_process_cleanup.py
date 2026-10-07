"""Exercise job cleanup with synthetic process tables; never signal host PIDs."""
from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[3]
CLEANUP = ROOT / "scripts/ci/cleanup-e2e-job-processes.sh"
ENTRYPOINT = ROOT / "scripts/ci/run-e2e-shell.sh"
BASH = shutil.which("bash")
START = "Wed Oct 7 10:00:00 2026"
LATER = "Wed Oct 7 10:00:01 2026"

# All relevant commands are exported shell functions, including kill (a bash
# builtin). Fake PID numbers are never passed to a real kill, ps, id, or sleep.
SHIM = r'''
ps() { "$TEST_PYTHON" "$TEST_FAKE" ps "$$" "$@"; }
id() { "$TEST_PYTHON" "$TEST_FAKE" id "$$" "$@"; }
uname() { printf '%s\n' "$TEST_PLATFORM"; }
kill() { "$TEST_PYTHON" "$TEST_FAKE" kill "$$" "$@"; }
sleep() { :; }
export -f ps id uname kill sleep
export BIFROST_E2E_JOB_ROOT_PID="$$"
export BIFROST_E2E_JOB_ROOT_START="$TEST_ROOT_START"
if [[ "${TEST_BAD_PARENT:-0}" == 1 ]]; then
  export BIFROST_E2E_JOB_ROOT_PID=500099
fi
bash "$TEST_CLEANUP"
status=$?
exit "$status"
'''

FAKE = r'''
import json, os, sys
from pathlib import Path
path = Path(os.environ["TEST_STATE"])
data = json.loads(path.read_text())
command, caller, *args = sys.argv[1:]
root = os.environ["BIFROST_E2E_JOB_ROOT_PID"]
start = "Wed Oct 7 10:00:00 2026"
rows = {
    "1": [0, 0, start, "S"],
    "10": [501, 1, start, "S"],
    root: [501, 10, start, "S"],
    caller: [501, int(root), start, "S"],
    "500090": [501, int(caller), start, "S"],
}
rows.update(data.get("rows", {}))
for row in rows.values():
    if row[1] == "ROOT":
        row[1] = int(root)
if command == "id":
    assert args == ["-u"], args
    print(data.get("uid", "501"))
    sys.exit(data.get("id_status", 0))
if command == "kill":
    assert len(args) == 2 and args[0] in ("-TERM", "-KILL"), args
    data.setdefault("signals", []).append(args)
    path.write_text(json.dumps(data))
    sys.exit(0)
assert command == "ps", command
if args == ["-axo", "pid="]:
    print("\n".join(pid for pid in rows if pid not in data.get("omitted", [])) + "\n" + data.get("extra_snapshot", ""))
    sys.exit(data.get("snapshot_status", 0))
assert len(args) == 4 and args[0] == "-p" and args[2:] == ["-o", "uid=,ppid=,lstart=,state="], args
pid = args[1]
reads = data.setdefault("reads", {})
reads[pid] = reads.get(pid, 0) + 1
for change in data.get("changes", []):
    if change["pid"] == pid and reads[pid] >= change["read"]:
        rows[pid] = change.get("row")
path.write_text(json.dumps(data))
row = rows.get(pid)
# Replacements must preserve a valid PPID so identity/UID/state tests cannot
# accidentally pass through the unrelated malformed-record guard.
if isinstance(row, list) and row[1] == "ROOT":
    row[1] = int(root)
if row is None:
    sys.exit(1)
if isinstance(row, str):
    print(row)
else:
    print(" ".join(map(str, row)))
'''


def process(parent="ROOT", uid=501, start=START, state="S"):
    return [uid, parent, start, state]


@unittest.skipUnless(BASH, "bash is required")
class JobProcessCleanupTests(unittest.TestCase):
    def run_cleanup(self, platform="Linux", **options):
        with tempfile.TemporaryDirectory(prefix="bifrost-cleanup-contract-") as tmp:
            directory = Path(tmp)
            fake = directory / "fake.py"
            fake.write_text(FAKE)
            state = directory / "state.json"
            state.write_text(json.dumps(options))
            env = dict(os.environ, GITHUB_ACTIONS=options.pop("actions", "true"),
                       TEST_PYTHON=sys.executable, TEST_FAKE=str(fake),
                       TEST_STATE=str(state), TEST_PLATFORM=platform,
                       TEST_ROOT_START=options.pop("root_start", START),
                       TEST_BAD_PARENT=str(options.pop("bad_parent", 0)),
                       TEST_CLEANUP=str(CLEANUP))
            result = subprocess.run([BASH, "-c", SHIM], cwd=directory, env=env,
                                    text=True, capture_output=True, timeout=15)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            return json.loads(state.read_text()), result.stdout

    def test_only_proven_live_descendants_receive_individual_term_and_kill(self):
        for platform in ("Linux", "Darwin"):
            with self.subTest(platform=platform):
                data, _ = self.run_cleanup(platform, rows={
                    "500001": process(),
                    "500002": process(parent=500001),
                    # A same-UID service started during E2E is not owned.
                    "500003": process(parent=1, start=LATER),
                    "500004": process(parent="ROOT", uid=0),
                    "500005": process(parent="ROOT", state="Z"),
                    "500006": process(parent=500004),
                    "500007": process(state="X"),
                    "500008": process(state="?"),
                })
                self.assertEqual(data["signals"], [
                    ["-TERM", "500001"], ["-TERM", "500002"],
                    ["-KILL", "500001"], ["-KILL", "500002"],
                ])
                # The caller, all its ancestors, and the cleanup subtree were
                # present in every mocked snapshot, but never signalled.

    def test_old_baseline_is_not_ownership_evidence_for_system_services(self):
        for platform in ("Linux", "Darwin"):
            with self.subTest(platform=platform):
                data, output = self.run_cleanup(platform, rows={
                    "500001": process(parent=1),  # swtransparencyd
                    "500002": process(parent=1),  # SecurityAgent
                    "500003": process(parent=10),  # RemoteManagementAgent
                })
                self.assertEqual(data.get("signals", []), [])
                self.assertIn("no proven E2E descendants", output)

    def test_changed_pid_identity_or_ownership_before_term_is_skipped(self):
        for platform in ("Linux", "Darwin"):
            for replacement in (None, process(start=LATER), process(parent=1),
                                process(uid=0), process(state="Z")):
                with self.subTest(platform=platform, replacement=replacement):
                    data, _ = self.run_cleanup(platform, rows={"500001": process()},
                                              changes=[{"pid": "500001", "read": 2,
                                                        "row": replacement}])
                    self.assertEqual(data.get("signals", []), [])

    def test_identity_and_ownership_are_rechecked_immediately_before_kill(self):
        for platform in ("Linux", "Darwin"):
            for replacement in (None, process(start=LATER), process(parent=1),
                                process(uid=0), process(state="Z")):
                with self.subTest(platform=platform, replacement=replacement):
                    data, _ = self.run_cleanup(platform, rows={"500001": process()},
                                              changes=[{"pid": "500001", "read": 8,
                                                        "row": replacement}])
                    self.assertEqual(data["signals"], [["-TERM", "500001"]])
                    self.assertEqual(data["reads"]["500001"], 8)

    def test_unverifiable_ancestry_and_malformed_records_fail_closed(self):
        data, _ = self.run_cleanup(rows={
            "500001": process(parent=500002),
            "500002": process(parent=500001),  # Cycle
            "500003": process(parent=500004),  # Missing parent
            "500005": "501 2",                # Missing start identity
            "500006": "501 2 malformed date S",
            "500007": "501 2 Wed Oct 7 10:00:00 2026 S extra",
            "500008": "501 2 Wed Oct 7 10:00:00 2026 S\n501 2 " + START + " S",
        }, extra_snapshot="0\n-1\nnot-a-pid\n500001 500002")
        self.assertEqual(data.get("signals", []), [])

    def test_parent_chain_must_still_prove_ownership_before_each_signal(self):
        for platform in ("Linux", "Darwin"):
            for read, expected in ((2, []), (8, [["-TERM", "500001"]])):
                with self.subTest(platform=platform, read=read):
                    # Parent is observed only as an ancestor, not a candidate.
                    data, _ = self.run_cleanup(platform, rows={
                        "500001": process(parent=500002),
                        "500002": process(),
                    }, omitted=["500002"], changes=[{
                        "pid": "500002", "read": read, "row": process(parent=1),
                    }])
                    self.assertEqual(data.get("signals", []), expected)

    def test_missing_or_mismatched_root_identity_disables_cleanup(self):
        for start in ("", LATER, "malformed"):
            with self.subTest(start=start):
                data, _ = self.run_cleanup(root_start=start, rows={"500001": process()})
                self.assertEqual(data.get("signals", []), [])
        data, _ = self.run_cleanup(bad_parent=1, rows={"500001": process()})
        self.assertEqual(data.get("signals", []), [])

    def test_disabled_unsupported_or_failed_snapshot_is_noop(self):
        for options in ({"actions": "false"}, {"platform": "FreeBSD"},
                        {"snapshot_status": 1}, {"uid": "invalid"}, {"id_status": 1}):
            with self.subTest(options=options):
                data, _ = self.run_cleanup(rows={"500001": process()}, **options)
                self.assertEqual(data.get("signals", []), [])

    def test_entrypoint_records_live_root_and_never_user_wide_baseline(self):
        source = ENTRYPOINT.read_text()
        self.assertIn('export BIFROST_E2E_JOB_ROOT_PID="$$"', source)
        self.assertIn('LC_ALL=C ps -p "$$" -o lstart=', source)
        self.assertNotIn("PROCESS_BASELINE", source)
        self.assertNotIn("ps -axo", source)
        self.assertIn("trap cleanup_tracked_e2e_processes EXIT", source)
        self.assertIn("--ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build", source)


if __name__ == "__main__":
    unittest.main()
