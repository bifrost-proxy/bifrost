#!/usr/bin/env bash

set -uo pipefail

# Being new and having the runner's UID is NOT proof of E2E ownership: macOS
# launches user services during the job too. Only reap current descendants of
# the still-running E2E entrypoint. Unverifiable/reparented orphans are left for
# their fixture cleanup or disposable runner teardown, never guessed by name.
root_pid="${BIFROST_E2E_JOB_ROOT_PID:-}"
root_start="${BIFROST_E2E_JOB_ROOT_START:-}"
if [[ "${GITHUB_ACTIONS:-}" != "true" || ! "$root_pid" =~ ^[0-9]+$ ||
  "$root_pid" -le 1 || "$root_pid" != "$PPID" || -z "$root_start" ]]; then
  exit 0
fi
case "$(uname -s 2>/dev/null)" in
  Darwin | Linux) ;;
  *) exit 0 ;;
esac
current_uid="$(id -u 2>/dev/null)" || exit 0
[[ "$current_uid" =~ ^[0-9]+$ ]] || exit 0

# lstart is supported on both Darwin and Linux. Keep it with the PID and check
# it again before EACH individual signal, including escalation to KILL.
read_process() {
  local record weekday month day clock year extra
  record="$(LC_ALL=C ps -p "$1" -o uid=,ppid=,lstart=,state= 2>/dev/null)" || return 1
  [[ "$record" != *$'\n'* ]] || return 1
  read -r process_uid process_ppid weekday month day clock year process_state extra <<<"$record"
  [[ "$process_uid" =~ ^[0-9]+$ && "$process_ppid" =~ ^[0-9]+$ &&
    "$weekday" =~ ^[A-Z][a-z][a-z]$ && "$month" =~ ^[A-Z][a-z][a-z]$ &&
    "$day" =~ ^[0-9]+$ && "$clock" =~ ^[0-9][0-9]:[0-9][0-9]:[0-9][0-9]$ &&
    "$year" =~ ^[0-9][0-9][0-9][0-9]$ && -n "$process_state" &&
    "$process_state" =~ ^[DIRSTUWt] && -z "$extra" ]] || return 1
  process_start="$weekday $month $day $clock $year"
}

is_owned_descendant() {
  local pid="$1" expected_start="${2:-}" visited=" "
  candidate_start=""
  # $$ and everything below this cleanup shell are never test candidates.
  # Requiring a path to our direct caller also excludes that caller, all its
  # ancestors, unrelated same-user services, and every foreign-UID process.
  [[ "$pid" != "$root_pid" ]] || return 1
  while [[ "$pid" =~ ^[0-9]+$ && "$pid" -gt 1 ]]; do
    [[ "$pid" != "$$" && "$visited" != *" $pid "* ]] || return 1
    visited+="$pid "
    read_process "$pid" || return 1
    [[ "$process_uid" == "$current_uid" ]] || return 1
    if [[ "$pid" == "$root_pid" ]]; then
      [[ "$process_start" == "$root_start" ]]
      return
    fi
    if [[ -z "$candidate_start" ]]; then
      candidate_start="$process_start"
      [[ -z "$expected_start" || "$candidate_start" == "$expected_start" ]] || return 1
    fi
    pid="$process_ppid"
  done
  return 1
}

candidate_pids=()
candidate_starts=()
# Failure to list processes cannot authorize a partial/guessed cleanup.
snapshot="$(ps -axo pid= 2>/dev/null)" || exit 0
while read -r pid; do
  if is_owned_descendant "$pid"; then
    candidate_pids+=("$pid")
    candidate_starts+=("$candidate_start")
  fi
done <<<"$snapshot"

if [[ "${#candidate_pids[@]}" -eq 0 ]]; then
  echo "[CLEANUP] no proven E2E descendants remain"
  exit 0
fi

for i in "${!candidate_pids[@]}"; do
  pid="${candidate_pids[$i]}"
  if is_owned_descendant "$pid" "${candidate_starts[$i]}"; then
    echo "[CLEANUP] terminating proven E2E descendant pid=$pid"
    kill -TERM "$pid" 2>/dev/null || true
  fi
done

for _ in 1 2 3 4 5; do
  any_remaining=0
  for i in "${!candidate_pids[@]}"; do
    if is_owned_descendant "${candidate_pids[$i]}" "${candidate_starts[$i]}"; then
      any_remaining=1
      break
    fi
  done
  [[ "$any_remaining" -eq 0 ]] && exit 0
  sleep 1
done

for i in "${!candidate_pids[@]}"; do
  pid="${candidate_pids[$i]}"
  if is_owned_descendant "$pid" "${candidate_starts[$i]}"; then
    echo "[CLEANUP] force-killing proven E2E descendant pid=$pid"
    kill -KILL "$pid" 2>/dev/null || true
  fi
done
