function Wait-UpgradeParentExit([int]$ParentPid, [int]$TimeoutMilliseconds) {
  $parent = Get-Process -Id $ParentPid -ErrorAction SilentlyContinue
  if (-not $parent) { return }
  try {
    # A retained Windows process handle can leave an exited PID discoverable.
    # Wait on the captured process, never infer liveness from another PID lookup.
    if (-not $parent.WaitForExit($TimeoutMilliseconds)) {
      throw "parent process $ParentPid did not exit before timeout"
    }
  } finally {
    $parent.Dispose()
  }
}
