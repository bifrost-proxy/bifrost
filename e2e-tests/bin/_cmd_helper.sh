#!/usr/bin/env bash

find_real_cmd() {
  local cmd_name="$1"
  local shim_dir
  shim_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  local path_entry candidate
  IFS=':' read -r -a _path_entries <<< "${PATH:-}"
  for path_entry in "${_path_entries[@]}"; do
    [[ -z "$path_entry" ]] && continue
    [[ "$path_entry" == "$shim_dir" ]] && continue
    candidate="$path_entry/$cmd_name"
    if [[ -x "$candidate" ]]; then
      echo "$candidate"
      return 0
    fi
  done
  return 1
}
