#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
HOOKS_DIR="${REPO_ROOT}/.githooks"
PRE_COMMIT_HOOK="${HOOKS_DIR}/pre-commit"

if ! git -C "${REPO_ROOT}" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "ERROR: ${0##*/} must be run from a Git checkout." >&2
  exit 1
fi

if [[ ! -d "${HOOKS_DIR}" ]]; then
  echo "ERROR: ${HOOKS_DIR} does not exist." >&2
  exit 1
fi

if [[ ! -f "${PRE_COMMIT_HOOK}" ]]; then
  echo "ERROR: ${PRE_COMMIT_HOOK} is missing." >&2
  exit 1
fi

shopt -s nullglob
hook_files=("${HOOKS_DIR}"/*)
if [[ "${#hook_files[@]}" -eq 0 ]]; then
  echo "ERROR: ${HOOKS_DIR} does not contain any hook files." >&2
  exit 1
fi

for hook_file in "${hook_files[@]}"; do
  [[ -f "${hook_file}" ]] || continue
  chmod +x "${hook_file}"
done

previous_hooks_path="$(git -C "${REPO_ROOT}" config --local --get core.hooksPath 2>/dev/null || true)"
git -C "${REPO_ROOT}" config --local core.hooksPath .githooks
current_hooks_path="$(git -C "${REPO_ROOT}" config --local --get core.hooksPath)"

echo "Git hooks initialized for ${REPO_ROOT}"
if [[ -n "${previous_hooks_path}" && "${previous_hooks_path}" != "${current_hooks_path}" ]]; then
  echo "Local hooksPath updated: ${previous_hooks_path} -> ${current_hooks_path}"
elif [[ "${previous_hooks_path}" == "${current_hooks_path}" ]]; then
  echo "Local hooksPath already configured: ${current_hooks_path}"
else
  echo "Local hooksPath configured: ${current_hooks_path}"
fi

echo "Executable hooks:"
for hook_file in "${hook_files[@]}"; do
  [[ -f "${hook_file}" ]] || continue
  printf '  - %s\n' "${hook_file#"${REPO_ROOT}/"}"
done

echo "Git will now run .githooks/pre-commit before each commit."
