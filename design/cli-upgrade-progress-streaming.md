# CLI upgrade progress streaming

## Background

`bifrost upgrade` already renders byte-level progress while it downloads the CLI archive. After the
CLI phase, however, it launches `bifrost app upgrade` with stdout and stderr redirected to temporary
files. The child can spend more than a minute downloading and installing the Desktop package, while
the parent terminal only shows `Updating Bifrost desktop app...` and the final success line.

The Desktop updater also writes byte-level download progress only to `upgrade-progress.json`, and
platform installer commands capture their output for error reporting without forwarding it to the
terminal. These layers combine into a long, silent interval that looks like a hung upgrade.

## User goal checklist

### Must implement

- Keep CLI archive percentage, transferred bytes, total bytes, and speed visible during download.
- Stream the Desktop child process stdout and stderr to the parent `bifrost upgrade` terminal while
  the child is still running.
- Render Desktop package byte-level download progress in the child terminal so the parent can relay
  it.
- Show Desktop install and restart stages immediately, and emit an elapsed-time heartbeat for a
  long-running platform installer.
- Stream useful platform installer stdout/stderr without losing it from the eventual failure reason.
- Stream Homebrew reinstall output for Homebrew-managed CLI installations.

### Must not break

- Preserve timeout handling and child termination.
- Preserve captured stdout/stderr so non-zero child exits still report the most useful diagnostic.
- Preserve `upgrade-progress.json` ownership and phase/percent updates for WebUI, tray, and Desktop
  handoff flows.
- Preserve non-interactive command exit codes and make redirected output usable as an append-only
  log; carriage-return download rendering may remain compatible with the existing CLI behavior.
- Do not touch the user's real `9900` proxy, real Desktop installation, or system proxy in tests.

### Must verify

- Unit tests prove incremental forwarding does not duplicate bytes and retained output remains
  available for error summaries.
- Upgrade command timeout and stderr-preferred failure tests remain green.
- Shell E2E proves a marker written by a deliberately slow Desktop installer becomes visible in the
  parent log before `bifrost upgrade` exits.
- Human tests exercise isolated CLI and Desktop package paths and compare visible stage/progress
  output with expected behavior.
- Remote CI enforces the workspace coverage gate and the existing upgrade matrix.

## Design

### Tee through independently reopened temporary files

The parent still captures child stdout/stderr in temporary files, but each child stream uses a
separately reopened file descriptor. The parent periodically reads only newly appended bytes and
forwards them to its own stdout/stderr. Independent file offsets are required: a cloned Unix file
descriptor can share an offset with the writer and would make concurrent tailing unsafe.

When the child exits or times out, the parent forwards the final bytes and then rereads the complete
files for structured failure summarization. Terminal forwarding errors are best-effort and never
replace the child result.

### Progress surfaces

- CLI HTTP archive: retain the existing 250ms carriage-return percentage/bytes/s renderer.
- Homebrew CLI install: inherit Homebrew reinstall stdout/stderr so formula download/build/install
  activity remains visible.
- Desktop HTTP package: print the same byte-level progress line every 250ms while continuing to
  write phase and percent to `upgrade-progress.json`.
- Desktop installer: print the install stage immediately, forward installer output, and print an
  elapsed-time heartbeat every five seconds while a silent installer remains active.
- Desktop restart/handoff: print the selected restart stage before launching or handing off.

## Test boundary

The temporal E2E builds a real DMG containing a temporary fake `.app` plus an incompressible payload,
serves it from a loopback HTTP server in 4 KiB chunks with delays, and installs it into a temporary
target. A PATH-scoped `ditto` wrapper also prints a marker and sleeps before invoking the system
`ditto`. The test starts `bifrost upgrade` in the background and separately proves that download
progress appears before the HTTP response completes and installer progress appears before the
parent exits. It finally reads the installed app's `Info.plist` and verifies the target version. No
real Desktop bundle, daemon, certificate, system proxy, or port 9900 state is modified.
