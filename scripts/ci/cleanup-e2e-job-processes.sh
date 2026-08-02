#!/usr/bin/env bash

set -uo pipefail

# Hosted runners execute one job per machine. The shell entrypoint snapshots the
# runner user's PIDs immediately before E2E; reap only same-user processes that
# appeared afterwards. This works on macOS where `ps` does not expose another
# process's RUNNER_TRACKING_ID environment, while still protecting pre-existing
# runner/system processes and this cleanup shell's complete ancestor chain.
baseline_file="${BIFROST_E2E_JOB_PROCESS_BASELINE:-}"
if [[ "${GITHUB_ACTIONS:-}" != "true" || -z "$baseline_file" || ! -f "$baseline_file" ]]; then
  exit 0
fi

case "$(uname -s 2>/dev/null)" in
  Darwin | Linux) ;;
  *) exit 0 ;;
esac

protected_pids=" "
current_pid="$$"
while [[ "$current_pid" =~ ^[0-9]+$ && "$current_pid" -gt 1 ]]; do
  protected_pids+="$current_pid "
  current_pid="$(ps -p "$current_pid" -o ppid= 2>/dev/null | tr -d '[:space:]' || true)"
done

baseline_pids=" "
while IFS= read -r pid; do
  [[ "$pid" =~ ^[0-9]+$ ]] && baseline_pids+="$pid "
done <"$baseline_file"

current_uid="$(id -u)"
candidate_pids=()
while read -r uid pid; do
  [[ "$uid" == "$current_uid" ]] || continue
  [[ "$pid" =~ ^[0-9]+$ ]] || continue
  [[ "$baseline_pids" != *" $pid "* ]] || continue
  [[ "$protected_pids" != *" $pid "* ]] || continue
  candidate_pids+=("$pid")
done < <(ps -axo uid=,pid= 2>/dev/null || true)

rm -f "$baseline_file" 2>/dev/null || true

# The `ps` used to build the snapshot can observe its own short-lived process.
# Drop candidates that have already exited before logging or signalling.
live_candidate_pids=()
for pid in "${candidate_pids[@]}"; do
  state="$(ps -p "$pid" -o state= 2>/dev/null | tr -d '[:space:]' || true)"
  [[ -n "$state" && "$state" != Z* ]] && live_candidate_pids+=("$pid")
done
if [[ "${#live_candidate_pids[@]}" -gt 0 ]]; then
  candidate_pids=("${live_candidate_pids[@]}")
else
  candidate_pids=()
fi

if [[ "${#candidate_pids[@]}" -eq 0 ]]; then
  echo "[CLEANUP] no tracked E2E child processes remain"
  exit 0
fi

echo "[CLEANUP] terminating ${#candidate_pids[@]} tracked E2E child process(es)"
for pid in "${candidate_pids[@]}"; do
  command_name="$(ps -p "$pid" -o comm= 2>/dev/null | tr -d '\r' || true)"
  echo "[CLEANUP] tracked pid=$pid command=${command_name:-unknown}"
done
kill -TERM "${candidate_pids[@]}" 2>/dev/null || true

for _ in 1 2 3 4 5; do
  remaining=()
  for pid in "${candidate_pids[@]}"; do
    state="$(ps -p "$pid" -o state= 2>/dev/null | tr -d '[:space:]' || true)"
    [[ -n "$state" && "$state" != Z* ]] && remaining+=("$pid")
  done
  [[ "${#remaining[@]}" -eq 0 ]] && exit 0
  candidate_pids=("${remaining[@]}")
  sleep 1
done

echo "[CLEANUP] force-killing ${#candidate_pids[@]} tracked E2E child process(es)"
kill -KILL "${candidate_pids[@]}" 2>/dev/null || true
