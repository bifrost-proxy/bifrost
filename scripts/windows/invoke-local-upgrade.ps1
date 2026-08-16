[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$AssetsDir,

    [string]$CliPath = $(Join-Path $env:LOCALAPPDATA 'bifrost\bin\bifrost.exe'),

    [ValidateRange(30, 1800)]
    [int]$TimeoutSeconds = 900
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$AssetsDir = [IO.Path]::GetFullPath($AssetsDir)
$CliPath = [IO.Path]::GetFullPath($CliPath)
if (-not (Test-Path $AssetsDir -PathType Container)) {
    throw "Assets directory does not exist: $AssetsDir"
}
if (-not (Test-Path $CliPath -PathType Leaf)) {
    throw "Installed CLI does not exist: $CliPath"
}

function Get-BifrostMachineRegistration {
    $roots = @(
        'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*',
        'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*'
    )
    $registrations = @(
        Get-ItemProperty -Path $roots -ErrorAction SilentlyContinue |
            Where-Object {
                $_.PSObject.Properties['DisplayName'] -and $_.DisplayName -eq 'Bifrost'
            }
    )
    if ($registrations.Count -ne 1) {
        throw "Expected exactly one machine-wide Bifrost MSI registration; found $($registrations.Count)"
    }
    return $registrations[0]
}

$beforeRegistration = Get-BifrostMachineRegistration
$beforeInstallLocation = [IO.Path]::GetFullPath([string]$beforeRegistration.InstallLocation).TrimEnd('\')
$desktopPath = Join-Path $beforeInstallLocation 'bifrost-desktop.exe'
$bundledCliPath = Join-Path $beforeInstallLocation 'resources\bin\bifrost.exe'
if (-not (Test-Path $desktopPath -PathType Leaf)) {
    throw "Registered Desktop executable does not exist: $desktopPath"
}
$beforeDesktopHash = (Get-FileHash -LiteralPath $desktopPath -Algorithm SHA256).Hash
$beforeDisplayVersion = [string]$beforeRegistration.DisplayVersion

$target = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') {
    'aarch64-pc-windows-msvc'
}
else {
    'x86_64-pc-windows-msvc'
}
$archivePattern = "bifrost-v*-$target.zip"
$archives = @(Get-ChildItem -LiteralPath $AssetsDir -File -Filter $archivePattern)
if ($archives.Count -ne 1) {
    throw "Expected exactly one $archivePattern in $AssetsDir; found $($archives.Count)"
}
$archive = $archives[0]
$archiveMatch = [regex]::Match(
    $archive.Name,
    "^bifrost-v(?<version>.+)-$([regex]::Escape($target))\.zip$"
)
if (-not $archiveMatch.Success) {
    throw "Could not derive the target version from $($archive.Name)"
}
$version = $archiveMatch.Groups['version'].Value
$desktopName = "bifrost-desktop-v$version-$target.msi"
$desktopPackage = Join-Path $AssetsDir $desktopName
if (-not (Test-Path $desktopPackage -PathType Leaf)) {
    throw "Matching Desktop MSI is missing: $desktopPackage"
}
if ($archive.Length -eq 0 -or (Get-Item $desktopPackage).Length -eq 0) {
    throw 'Local upgrade assets must be non-empty files'
}
$beforeCliHash = (Get-FileHash -LiteralPath $CliPath -Algorithm SHA256).Hash

$environmentKeys = @(
    'BIFROST_UPGRADE_TEST_ALLOW_RELEASE_OVERRIDES',
    'BIFROST_UPGRADE_TEST_LATEST_VERSION',
    'BIFROST_UPGRADE_TEST_ARCHIVE',
    'BIFROST_APP_UPGRADE_TEST_PACKAGE',
    'BIFROST_EXTERNAL_CLI_WORKER',
    'BIFROST_DETACHED_DAEMON_CHILD'
)
$previous = @{}
foreach ($key in $environmentKeys) {
    $previous[$key] = [Environment]::GetEnvironmentVariable($key, 'Process')
}

try {
    $env:BIFROST_UPGRADE_TEST_ALLOW_RELEASE_OVERRIDES = '1'
    $env:BIFROST_UPGRADE_TEST_LATEST_VERSION = $version
    $env:BIFROST_UPGRADE_TEST_ARCHIVE = $archive.FullName
    $env:BIFROST_APP_UPGRADE_TEST_PACKAGE = $desktopPackage
    Remove-Item Env:BIFROST_EXTERNAL_CLI_WORKER -ErrorAction SilentlyContinue
    Remove-Item Env:BIFROST_DETACHED_DAEMON_CHILD -ErrorAction SilentlyContinue

    Write-Host "Local update source: $AssetsDir" -ForegroundColor Cyan
    Write-Host "Installed CLI:      $CliPath"
    Write-Host "Target version:     $version"
    Write-Host "CLI archive:        $($archive.FullName)"
    Write-Host "Desktop package:    $desktopPackage"
    Write-Host ''

    & $CliPath upgrade -y
    $upgradeExit = $LASTEXITCODE
    if ($upgradeExit -ne 0) {
        throw "Local upgrade exited with code $upgradeExit"
    }

    # Windows self-replacement is deferred until this updater exits. Repeatedly
    # starting `bifrost --version` here can lock the target executable and
    # perturb the exact replacement path this script is meant to validate.
    # Observe the file hash instead, then execute the new CLI exactly once.
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    $afterCliHash = $beforeCliHash
    while ($afterCliHash -eq $beforeCliHash -and (Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 250
        try {
            $afterCliHash = (Get-FileHash -LiteralPath $CliPath -Algorithm SHA256).Hash
        }
        catch {
            # A short sharing violation while the helper swaps the executable
            # is expected. Keep waiting without spawning the CLI.
        }
    }
    if ($afterCliHash -eq $beforeCliHash) {
        throw "Timed out after $TimeoutSeconds seconds waiting for the installed CLI file to change"
    }

    $updatedVersion = (& $CliPath --version 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw 'Updated CLI version verification failed'
    }
    if ($updatedVersion -ne "bifrost $version") {
        throw "Updated CLI reported an unexpected version: $updatedVersion"
    }

    $afterRegistration = Get-BifrostMachineRegistration
    $afterInstallLocation = [IO.Path]::GetFullPath([string]$afterRegistration.InstallLocation).TrimEnd('\')
    if ($afterInstallLocation -ne $beforeInstallLocation) {
        throw "Desktop install location drifted from $beforeInstallLocation to $afterInstallLocation"
    }
    if (-not (Test-Path $desktopPath -PathType Leaf)) {
        throw "Updated Desktop executable is missing from $desktopPath"
    }
    $afterDesktopHash = (Get-FileHash -LiteralPath $desktopPath -Algorithm SHA256).Hash
    if ($afterDesktopHash -eq $beforeDesktopHash) {
        throw 'Desktop executable did not change during the local update'
    }
    $afterDisplayVersion = [string]$afterRegistration.DisplayVersion
    if ($afterDisplayVersion -eq $beforeDisplayVersion) {
        throw "Desktop MSI registration did not change from $beforeDisplayVersion"
    }
    if (-not (Test-Path $bundledCliPath -PathType Leaf)) {
        throw "Updated Desktop bundled CLI is missing from $bundledCliPath"
    }
    $bundledVersion = (& $bundledCliPath --version 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $bundledVersion -ne "bifrost $version") {
        throw "Desktop bundled CLI reported an unexpected version: $bundledVersion"
    }
    Write-Host "Updated CLI:        $updatedVersion" -ForegroundColor Green
    Write-Host "Updated Desktop:    $afterDisplayVersion at $afterInstallLocation" -ForegroundColor Green
}
finally {
    foreach ($key in $environmentKeys) {
        $value = $previous[$key]
        if ($null -eq $value) {
            [Environment]::SetEnvironmentVariable($key, $null, 'Process')
        }
        else {
            [Environment]::SetEnvironmentVariable($key, $value, 'Process')
        }
    }
}
