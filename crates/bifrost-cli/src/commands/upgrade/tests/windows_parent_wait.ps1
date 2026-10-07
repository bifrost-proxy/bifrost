
function Start-OwnedParent([string]$Command) {
  $start = New-Object System.Diagnostics.ProcessStartInfo
  $start.FileName = Join-Path $PSHOME "powershell.exe"
  $start.Arguments = "-NoProfile -NonInteractive -Command `"$Command`""
  $start.UseShellExecute = $false
  $start.CreateNoWindow = $true
  $start.RedirectStandardInput = $true
  $start.RedirectStandardOutput = $true
  $start.RedirectStandardError = $true
  return [System.Diagnostics.Process]::Start($start)
}

function Close-OwnedParent($Process) {
  if (-not $Process) { return }
  try {
    if (-not $Process.HasExited) { $Process.Kill() }
    if (-not $Process.WaitForExit(10000)) { throw "fixture child did not exit" }
  } finally {
    $Process.Dispose()
  }
}

# Capture a real native process handle before allowing the owned child to exit.
# Stub only PID discovery so exited-process enumeration differences cannot skip
# this branch. The old post-wait PID lookup would return this object and fail.
$exited = Start-OwnedParent "[Console]::ReadLine() | Out-Null; exit 0"
$captured = $null
try {
  $captured = [System.Diagnostics.Process]::GetProcessById($exited.Id)
  $retainedHandle = $captured.Handle
  $exited.StandardInput.WriteLine("exit")
  $exited.StandardInput.Close()
  if (-not $exited.WaitForExit(10000)) { throw "exit fixture did not exit" }
  if (-not $captured.HasExited) { throw "retained fixture must already be exited" }
  $script:retainedParent = $captured
  $script:parentLookups = 0
  function Get-Process {
    [CmdletBinding()]
    param([int]$Id)
    if ($Id -ne $exited.Id) { throw "unexpected fixture PID lookup" }
    $script:parentLookups++
    return $script:retainedParent
  }
  Wait-UpgradeParentExit $exited.Id 200
  if ($script:parentLookups -ne 1) { throw "helper must resolve the parent once" }
  Write-Output "retained-exit-ok"
} finally {
  Remove-Item Function:Get-Process -ErrorAction SilentlyContinue
  if ($captured) { $captured.Dispose() }
  Close-OwnedParent $exited
}

# A genuinely live parent must fail within its configured wait, and stay alive.
$live = Start-OwnedParent "[Console]::ReadLine() | Out-Null"
try {
  $clock = [System.Diagnostics.Stopwatch]::StartNew()
  $failure = $null
  try { Wait-UpgradeParentExit $live.Id 200 } catch { $failure = $_.Exception.Message }
  $clock.Stop()
  if ($failure -ne "parent process $($live.Id) did not exit before timeout") {
    throw "live parent did not report the bounded timeout: $failure"
  }
  if ($clock.ElapsedMilliseconds -lt 100 -or $clock.ElapsedMilliseconds -gt 5000) {
    throw "live parent wait escaped its test bound: $($clock.ElapsedMilliseconds)ms"
  }
  if ($live.HasExited) { throw "waiting must not terminate the parent" }
  Write-Output "live-timeout-ok"
} finally {
  Close-OwnedParent $live
}

$finishing = Start-OwnedParent "Start-Sleep -Milliseconds 200; exit 0"
try {
  Wait-UpgradeParentExit $finishing.Id 10000
  if (-not $finishing.HasExited) { throw "helper returned before parent exit" }
  Write-Output "exit-during-wait-ok"
} finally {
  Close-OwnedParent $finishing
}

# An already-disappeared parent needs no wait. Keep this deterministic without
# depending on a hard-coded PID remaining unused on the host.
try {
  function Get-Process {
    [CmdletBinding()]
    param([int]$Id)
    return $null
  }
  Wait-UpgradeParentExit 1234 200
  Write-Output "missing-parent-ok"
} finally {
  Remove-Item Function:Get-Process -ErrorAction SilentlyContinue
}
