[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$')]
    [string]$Version,

    [ValidateSet('aarch64-pc-windows-msvc', 'x86_64-pc-windows-msvc')]
    [string]$Target = $(
        if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') {
            'aarch64-pc-windows-msvc'
        }
        else {
            'x86_64-pc-windows-msvc'
        }
    ),

    [string]$OutputDir,

    [switch]$SkipFrontend
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
if (-not $OutputDir) {
    $OutputDir = Join-Path $repoRoot ".bifrost-local-upgrade\$Version-$Target"
}
elseif (-not [IO.Path]::IsPathRooted($OutputDir)) {
    $OutputDir = Join-Path $repoRoot $OutputDir
}
$OutputDir = [IO.Path]::GetFullPath($OutputDir)

function Invoke-Checked {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Program,
        [Parameter(ValueFromRemainingArguments = $true)]
        [string[]]$Arguments
    )
    Write-Host "> $Program $($Arguments -join ' ')" -ForegroundColor Cyan
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Program exited with code $LASTEXITCODE"
    }
}

foreach ($command in @('cargo', 'node', 'pnpm')) {
    if (-not (Get-Command $command -ErrorAction SilentlyContinue)) {
        throw "Required command is unavailable: $command"
    }
}

$tauriConfig = Join-Path $repoRoot 'desktop\src-tauri\tauri.conf.json'
$rootCargoLock = Join-Path $repoRoot 'Cargo.lock'
$desktopCargoManifest = Join-Path $repoRoot 'desktop\src-tauri\Cargo.toml'
$desktopCargoLock = Join-Path $repoRoot 'desktop\src-tauri\Cargo.lock'
$tauriSchemaDir = Join-Path $repoRoot 'desktop\src-tauri\gen\schemas'
$originalTauriConfig = [IO.File]::ReadAllBytes($tauriConfig)
$originalRootCargoLock = [IO.File]::ReadAllBytes($rootCargoLock)
$originalDesktopCargoManifest = [IO.File]::ReadAllBytes($desktopCargoManifest)
$originalDesktopCargoLock = [IO.File]::ReadAllBytes($desktopCargoLock)
$previousVersion = $env:BIFROST_VERSION
$previousSkipFrontend = $env:SKIP_FRONTEND_BUILD
$scratch = Join-Path ([IO.Path]::GetTempPath()) ("bifrost-local-assets-" + [Guid]::NewGuid())
$tauriSchemaBackup = Join-Path $scratch 'original-tauri-schemas'
$tauriSchemaDirExisted = Test-Path $tauriSchemaDir -PathType Container

try {
    New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
    New-Item -ItemType Directory -Force -Path $scratch | Out-Null
    if ($tauriSchemaDirExisted) {
        Copy-Item -LiteralPath $tauriSchemaDir -Destination $tauriSchemaBackup -Recurse -Force
    }
    Set-Location $repoRoot
    $env:BIFROST_VERSION = $Version
    $env:SKIP_FRONTEND_BUILD = '1'

    $rootTauriCli = Join-Path $repoRoot 'node_modules\.bin\tauri.cmd'
    $webTypeScript = Join-Path $repoRoot 'web\node_modules\.bin\tsc.cmd'
    if (-not (Test-Path $rootTauriCli -PathType Leaf)) {
        Invoke-Checked pnpm 'install' '--frozen-lockfile'
    }
    if (-not (Test-Path $webTypeScript -PathType Leaf)) {
        Invoke-Checked pnpm '--dir' 'web' 'install' '--frozen-lockfile'
    }

    if ($SkipFrontend) {
        $frontendDist = Join-Path $repoRoot 'web\dist-desktop'
        if (-not (Test-Path $frontendDist -PathType Container)) {
            throw "-SkipFrontend requires an existing desktop frontend build: $frontendDist"
        }
    }
    else {
        Invoke-Checked pnpm '--dir' 'web' 'run' 'build:desktop'
    }

    Invoke-Checked cargo 'build' '-p' 'bifrost-cli' '--release' '--target' $Target
    $builtCli = Join-Path $repoRoot "target\$Target\release\bifrost.exe"
    $builtCliVersion = (& $builtCli --version 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $builtCliVersion -ne "bifrost $Version") {
        throw "Built CLI version mismatch: expected 'bifrost $Version', got '$builtCliVersion'"
    }
    Write-Host "Verified built CLI: $builtCliVersion" -ForegroundColor Green
    Invoke-Checked node 'scripts/prepare-tauri-sidecar.mjs' 'release' $Target
    Invoke-Checked node 'scripts/sync-tauri-version.mjs' '--msi'
    $expectedMsiVersion = (Get-Content -LiteralPath $tauriConfig -Raw | ConvertFrom-Json).version
    $bundleMsiDir = Join-Path $repoRoot "desktop\src-tauri\target\$Target\release\bundle\msi"
    if (Test-Path $bundleMsiDir -PathType Container) {
        # Tauri keeps prior versioned MSI files in this generated directory.
        # Remove only this target's generated MSI outputs so a repeated local
        # build cannot rename an older package as the requested local version.
        Remove-Item -LiteralPath $bundleMsiDir -Recurse -Force
    }
    Invoke-Checked pnpm 'exec' 'tauri' 'build' '--config' 'desktop/src-tauri/tauri.conf.json' '--target' $Target '--bundles' 'msi'

    $archiveName = "bifrost-v$Version-$Target"
    $archiveDir = Join-Path $scratch $archiveName
    New-Item -ItemType Directory -Force -Path $archiveDir | Out-Null
    Copy-Item $builtCli $archiveDir
    $readme = Join-Path $repoRoot 'README.md'
    if (Test-Path $readme -PathType Leaf) {
        Copy-Item $readme $archiveDir
    }
    $archiveOutput = Join-Path $OutputDir "$archiveName.zip"
    Compress-Archive -Path $archiveDir -DestinationPath $archiveOutput -Force

    $msiCandidates = @(Get-ChildItem -LiteralPath $bundleMsiDir -File -Filter '*.msi')
    if ($msiCandidates.Count -ne 1) {
        throw "Expected exactly one Desktop MSI in $bundleMsiDir; found $($msiCandidates.Count)"
    }
    $msi = $msiCandidates[0]
    if ($msi.Name -notlike "*$expectedMsiVersion*") {
        throw "Desktop MSI filename does not contain expected version ${expectedMsiVersion}: $($msi.Name)"
    }
    $desktopOutput = Join-Path $OutputDir "bifrost-desktop-v$Version-$Target.msi"
    Copy-Item $msi.FullName $desktopOutput -Force

    $archiveHash = (Get-FileHash $archiveOutput -Algorithm SHA256).Hash.ToLowerInvariant()
    $desktopHash = (Get-FileHash $desktopOutput -Algorithm SHA256).Hash.ToLowerInvariant()
    [IO.File]::WriteAllText("$archiveOutput.sha256", "$archiveHash  $([IO.Path]::GetFileName($archiveOutput))`n")
    [IO.File]::WriteAllText("$desktopOutput.sha256", "$desktopHash  $([IO.Path]::GetFileName($desktopOutput))`n")

    Write-Host ''
    Write-Host 'Local upgrade assets are ready:' -ForegroundColor Green
    Write-Host "  $OutputDir"
    Write-Host "  CLI:     $archiveOutput"
    Write-Host "  Desktop: $desktopOutput"
}
finally {
    # Cargo/Tauri may refresh lockfile package metadata or normalize source
    # file line endings while building. Local asset generation must leave the
    # caller's working tree byte-for-byte unchanged.
    [IO.File]::WriteAllBytes($tauriConfig, $originalTauriConfig)
    [IO.File]::WriteAllBytes($rootCargoLock, $originalRootCargoLock)
    [IO.File]::WriteAllBytes($desktopCargoManifest, $originalDesktopCargoManifest)
    [IO.File]::WriteAllBytes($desktopCargoLock, $originalDesktopCargoLock)
    if (Test-Path $tauriSchemaDir -PathType Container) {
        Remove-Item -LiteralPath $tauriSchemaDir -Recurse -Force
    }
    if ($tauriSchemaDirExisted) {
        Copy-Item -LiteralPath $tauriSchemaBackup -Destination $tauriSchemaDir -Recurse -Force
    }
    if ($null -eq $previousVersion) {
        Remove-Item Env:BIFROST_VERSION -ErrorAction SilentlyContinue
    }
    else {
        $env:BIFROST_VERSION = $previousVersion
    }
    if ($null -eq $previousSkipFrontend) {
        Remove-Item Env:SKIP_FRONTEND_BUILD -ErrorAction SilentlyContinue
    }
    else {
        $env:SKIP_FRONTEND_BUILD = $previousSkipFrontend
    }
    if (Test-Path $scratch -PathType Container) {
        Remove-Item -LiteralPath $scratch -Recurse -Force
    }
    Set-Location $repoRoot
}
