#!/usr/bin/env pwsh
#Requires -Version 5.1

<#
.SYNOPSIS
    Bifrost installation script for Windows
.DESCRIPTION
    Downloads and installs the Bifrost CLI and desktop app
.PARAMETER Version
    Specific version to install (e.g., v0.1.0). If not specified, installs the latest version.
.PARAMETER InstallDir
    Installation directory. Defaults to $env:LOCALAPPDATA\bifrost\bin
.PARAMETER NoDesktop
    Skip automatic installation of the Bifrost desktop app
.EXAMPLE
    irm https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.ps1 | iex
.EXAMPLE
    .\install-binary.ps1 -Version v0.0.9-alpha
.EXAMPLE
    .\install-binary.ps1 -InstallDir "C:\Tools\bifrost"
#>

param(
    [string]$Version = "",
    [string]$InstallDir = "",
    [switch]$NoDesktop
)

$ErrorActionPreference = "Stop"

$REPO = "bifrost-proxy/bifrost"
$BINARY_NAME = "bifrost"
$DEFAULT_GITHUB_MIRROR_URLS = @(
    "https://github.com",
    "https://ghfast.top/https://github.com",
    "https://github.moeyy.xyz/https://github.com"
)

if (-not $InstallDir) {
    $InstallDir = Join-Path $env:LOCALAPPDATA "bifrost\bin"
}

function Write-Banner {
    Write-Host ""
    Write-Host "+---------------------------------------------------------+" -ForegroundColor Cyan
    Write-Host "|                                                         |" -ForegroundColor Cyan
    Write-Host "|   ____  _  __                _                          |" -ForegroundColor Cyan
    Write-Host "|  |  _ \(_)/ _|_ __ ___  ___| |_                         |" -ForegroundColor Cyan
    Write-Host "|  | |_) | | |_| '__/ _ \/ __| __|                        |" -ForegroundColor Cyan
    Write-Host "|  |  _ <| |  _| | | (_) \__ \ |_                         |" -ForegroundColor Cyan
    Write-Host "|  |_| \_\_|_| |_|  \___/|___/\__|                        |" -ForegroundColor Cyan
    Write-Host "|                                                         |" -ForegroundColor Cyan
    Write-Host "|   High-performance HTTP/HTTPS/SOCKS5 Proxy Server       |" -ForegroundColor Cyan
    Write-Host "|                                                         |" -ForegroundColor Cyan
    Write-Host "+---------------------------------------------------------+" -ForegroundColor Cyan
    Write-Host ""
}

function Write-Step {
    param([string]$Message)
    Write-Host "==> " -ForegroundColor Blue -NoNewline
    Write-Host $Message
}

function Write-Success {
    param([string]$Message)
    Write-Host "[OK] " -ForegroundColor Green -NoNewline
    Write-Host $Message
}

function Write-Warning {
    param([string]$Message)
    Write-Host "[!] " -ForegroundColor Yellow -NoNewline
    Write-Host $Message
}

function Write-Error {
    param([string]$Message)
    Write-Host "[X] " -ForegroundColor Red -NoNewline
    Write-Host $Message
}

function Test-BifrostEnabled {
    param(
        [string]$Value,
        [bool]$Default = $true
    )

    if ([string]::IsNullOrWhiteSpace($Value)) {
        return $Default
    }

    return @("0", "false", "no", "off") -notcontains $Value.Trim().ToLowerInvariant()
}

function Install-BifrostDesktop {
    param(
        [string]$BinaryPath,
        [string]$TargetVersion
    )

    $autoDesktop = Test-BifrostEnabled -Value $env:BIFROST_INSTALL_AUTO_DESKTOP
    if ($NoDesktop -or -not $autoDesktop) {
        Write-Warning "Desktop app installation skipped"
        return
    }

    Write-Step "Installing desktop app..."
    $commandText = "$BinaryPath app install --version $TargetVersion --yes"
    if (Test-BifrostEnabled -Value $env:BIFROST_INSTALL_DESKTOP_DRY_RUN -Default $false) {
        Write-Host "  [dry-run] BIFROST_APP_SKIP_RESTART=1 $commandText"
        Write-Success "Desktop app installation planned"
        return
    }

    $previousSkipRestart = $env:BIFROST_APP_SKIP_RESTART
    try {
        $env:BIFROST_APP_SKIP_RESTART = "1"
        & $BinaryPath app install --version $TargetVersion --yes
        if ($LASTEXITCODE -ne 0) {
            throw "desktop installer exited with code $LASTEXITCODE"
        }
        Write-Success "Desktop app installed"
    }
    catch {
        Write-Warning "Desktop app installation failed; the CLI is still installed"
        Write-Warning "You can retry manually with:"
        Write-Host "  $commandText"
        Write-Warning $_.Exception.Message
    }
    finally {
        if ($null -eq $previousSkipRestart) {
            Remove-Item Env:BIFROST_APP_SKIP_RESTART -ErrorAction SilentlyContinue
        }
        else {
            $env:BIFROST_APP_SKIP_RESTART = $previousSkipRestart
        }
    }
}

function Install-BinaryAtomically {
    param(
        [string]$SourcePath,
        [string]$DestPath
    )

    $tempPath = "$DestPath.tmp.$PID"
    if (Test-Path $tempPath) {
        Remove-Item -Path $tempPath -Force
    }
    Copy-Item -Path $SourcePath -Destination $tempPath -Force
    Move-Item -Path $tempPath -Destination $DestPath -Force
}

function Split-PathList {
    param([string]$PathList)

    if ([string]::IsNullOrWhiteSpace($PathList)) {
        return @()
    }

    return @($PathList -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
}

function Normalize-PathEntry {
    param([string]$PathEntry)

    if ($null -eq $PathEntry) {
        return ""
    }

    return $PathEntry.Trim().TrimEnd('\')
}

function Test-PathListContains {
    param(
        [string]$PathList,
        [string]$Directory
    )

    $normalizedDirectory = Normalize-PathEntry -PathEntry $Directory
    foreach ($entry in @(Split-PathList -PathList $PathList)) {
        if ((Normalize-PathEntry -PathEntry $entry) -ieq $normalizedDirectory) {
            return $true
        }
    }

    return $false
}

function Add-PathListEntry {
    param(
        [string]$PathList,
        [string]$Directory
    )

    if ([string]::IsNullOrWhiteSpace($PathList)) {
        return $Directory
    }

    $normalizedDirectory = Normalize-PathEntry -PathEntry $Directory
    $entries = @()
    foreach ($entry in @(Split-PathList -PathList $PathList)) {
        if ((Normalize-PathEntry -PathEntry $entry) -ine $normalizedDirectory) {
            $entries += $entry
        }
    }

    if ($entries.Count -eq 0) {
        return $Directory
    }

    return "$Directory;$($entries -join ';')"
}

function Add-BifrostToUserPath {
    param([string]$Directory)

    $currentUserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $alreadyInUserPath = Test-PathListContains -PathList $currentUserPath -Directory $Directory

    $newUserPath = Add-PathListEntry -PathList $currentUserPath -Directory $Directory
    if ($newUserPath -ne $currentUserPath) {
        [Environment]::SetEnvironmentVariable("Path", $newUserPath, "User")
    }

    $env:Path = Add-PathListEntry -PathList $env:Path -Directory $Directory

    if ($alreadyInUserPath) {
        return "already"
    }

    return "added"
}

function Convert-ToBifrostArchitecture {
    param([object]$Architecture)

    if ($null -eq $Architecture) {
        return $null
    }

    $arch = ([string]$Architecture).Trim().ToUpperInvariant()
    switch ($arch) {
        "X64" { return "x86_64" }
        "AMD64" { return "x86_64" }
        "ARM64" { return "aarch64" }
        "AARCH64" { return "aarch64" }
        default { return $null }
    }
}

function Resolve-BifrostArchitecture {
    param(
        [object]$RuntimeArchitecture,
        [string]$NativeArchitecture,
        [string]$ProcessArchitecture,
        [bool]$Is64BitOperatingSystem
    )

    $runtimeArch = Convert-ToBifrostArchitecture $RuntimeArchitecture
    if ($runtimeArch) {
        return $runtimeArch
    }

    $nativeArch = Convert-ToBifrostArchitecture $NativeArchitecture
    if ($nativeArch) {
        return $nativeArch
    }

    $processArch = Convert-ToBifrostArchitecture $ProcessArchitecture
    if ($processArch) {
        return $processArch
    }

    if ($Is64BitOperatingSystem) {
        return "x86_64"
    }

    return "unknown"
}

function Get-Architecture {
    $runtimeArchitecture = $null
    try {
        $runtimeArchitecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    }
    catch {
        # Windows PowerShell 5.1 can run on hosts where RuntimeInformation is unavailable.
    }

    return Resolve-BifrostArchitecture `
        -RuntimeArchitecture $runtimeArchitecture `
        -NativeArchitecture $env:PROCESSOR_ARCHITEW6432 `
        -ProcessArchitecture $env:PROCESSOR_ARCHITECTURE `
        -Is64BitOperatingSystem ([Environment]::Is64BitOperatingSystem)
}

function Get-Target {
    param([string]$Arch)
    switch ($Arch) {
        "x86_64" { return "x86_64-pc-windows-msvc" }
        "aarch64" { return "aarch64-pc-windows-msvc" }
        default { return $null }
    }
}

function Get-GithubHeaders {
    $headers = @{}
    $token = $env:GITHUB_TOKEN
    if ($token) {
        $headers["Authorization"] = "token $token"
    }
    return $headers
}

function Get-IntEnv {
    param(
        [string]$Name,
        [int]$Default
    )

    $value = [Environment]::GetEnvironmentVariable($Name)
    if (-not $value) {
        return $Default
    }

    $parsed = 0
    if ([int]::TryParse($value, [ref]$parsed) -and $parsed -gt 0) {
        return $parsed
    }

    return $Default
}

function Get-GithubMirrorList {
    $preferred = $env:BIFROST_GITHUB_MIRROR
    $mirrors = New-Object System.Collections.Generic.List[string]

    if ($preferred) {
        [void]$mirrors.Add($preferred.TrimEnd('/'))
    }

    foreach ($mirror in $DEFAULT_GITHUB_MIRROR_URLS) {
        $normalized = $mirror.TrimEnd('/')
        if (-not $preferred -or $normalized -ne $preferred.TrimEnd('/')) {
            [void]$mirrors.Add($normalized)
        }
    }

    return $mirrors.ToArray()
}

function Get-MirrorDisplayName {
    param([string]$BaseUrl)

    return ($BaseUrl -replace '^https?://', '' -replace '/.*$', '')
}

function Join-GithubUrl {
    param(
        [string]$BaseUrl,
        [string]$GithubPath
    )

    return "$($BaseUrl.TrimEnd('/'))/$($GithubPath.TrimStart('/'))"
}

function Test-GithubUrl {
    param([string]$Url)

    $timeout = Get-IntEnv -Name "BIFROST_MIRROR_PROBE_TIMEOUT" -Default 5

    try {
        Invoke-WebRequest -Uri $Url -Method Head -MaximumRedirection 5 -TimeoutSec $timeout -UseBasicParsing -ErrorAction Stop | Out-Null
        return $true
    }
    catch {
        try {
            $headers = @{ Range = "bytes=0-0" }
            Invoke-WebRequest -Uri $Url -Headers $headers -MaximumRedirection 5 -TimeoutSec $timeout -UseBasicParsing -ErrorAction Stop | Out-Null
            return $true
        }
        catch {
            return $false
        }
    }
}

function Select-FastestGithubBase {
    param([string]$GithubPath)

    $mirrors = @(Get-GithubMirrorList)
    if ($mirrors.Count -eq 0) {
        return $null
    }

    if ($mirrors.Count -eq 1) {
        return $mirrors[0]
    }

    if ($env:BIFROST_INSTALLER_TEST_DISABLE_JOBS -eq "1") {
        foreach ($mirror in $mirrors) {
            $url = Join-GithubUrl -BaseUrl $mirror -GithubPath $GithubPath
            if (Test-GithubUrl -Url $url) {
                return $mirror
            }
        }
        return $null
    }

    $jobs = @()

    foreach ($mirror in $mirrors) {
        $url = Join-GithubUrl -BaseUrl $mirror -GithubPath $GithubPath
        $script = {
            param($Mirror, $Url, $Timeout)
            try {
                Invoke-WebRequest -Uri $Url -Method Head -MaximumRedirection 5 -TimeoutSec $Timeout -UseBasicParsing -ErrorAction Stop | Out-Null
                return $Mirror
            }
            catch {
                try {
                    $headers = @{ Range = "bytes=0-0" }
                    Invoke-WebRequest -Uri $Url -Headers $headers -MaximumRedirection 5 -TimeoutSec $Timeout -UseBasicParsing -ErrorAction Stop | Out-Null
                    return $Mirror
                }
                catch {
                    return $null
                }
            }
        }
        $timeout = Get-IntEnv -Name "BIFROST_MIRROR_PROBE_TIMEOUT" -Default 5
        $jobs += Start-Job -ScriptBlock $script -ArgumentList $mirror, $url, $timeout
    }

    try {
        $deadline = (Get-Date).AddSeconds((Get-IntEnv -Name "BIFROST_MIRROR_PROBE_TIMEOUT" -Default 5) + 1)
        while ((Get-Date) -lt $deadline) {
            foreach ($job in $jobs) {
                if ($job.State -eq "Completed") {
                    $result = Receive-Job -Job $job -ErrorAction SilentlyContinue
                    if ($result) {
                        return "$result"
                    }
                }
            }
            Start-Sleep -Milliseconds 200
        }
    }
    finally {
        foreach ($job in $jobs) {
            Stop-Job -Job $job -ErrorAction SilentlyContinue | Out-Null
            Remove-Job -Job $job -Force -ErrorAction SilentlyContinue | Out-Null
        }
    }

    return $null
}

function Get-LatestVersionViaRedirect {
    param([string]$BaseUrl = "https://github.com")

    $redirectUrl = Join-GithubUrl -BaseUrl $BaseUrl -GithubPath "$REPO/releases/latest"
    try {
        $response = Invoke-WebRequest -Uri $redirectUrl -MaximumRedirection 0 -TimeoutSec (Get-IntEnv -Name "BIFROST_MIRROR_PROBE_TIMEOUT" -Default 5) -UseBasicParsing -ErrorAction SilentlyContinue
        $location = $response.Headers["Location"]
    }
    catch {
        $location = $_.Exception.Response.Headers.Location
        if (-not $location) {
            $location = $_.Exception.Response.ResponseUri
        }
    }

    if ($location) {
        $locationStr = "$location"
        if ($locationStr -match '/tag/([^/]+)') {
            return $Matches[1]
        }
    }
    return $null
}

function Get-LatestVersionViaApi {
    $allReleasesUrl = "https://api.github.com/repos/$REPO/releases?per_page=10"
    $headers = Get-GithubHeaders

    try {
        $releases = Invoke-RestMethod -Uri $allReleasesUrl -Headers $headers -UseBasicParsing -ErrorAction Stop
    }
    catch {
        return $null
    }

    if (-not $releases -or $releases.Count -eq 0) {
        return $null
    }

    $stableRelease = $releases | Where-Object { -not $_.prerelease } | Select-Object -First 1
    if ($stableRelease) {
        return $stableRelease.tag_name
    }

    return $releases[0].tag_name
}

function Get-LatestVersion {
    $selectedBase = Select-FastestGithubBase -GithubPath "$REPO/releases/latest"
    if ($selectedBase) {
        $version = Get-LatestVersionViaRedirect -BaseUrl $selectedBase
        if ($version) {
            return $version
        }
    }

    foreach ($baseUrl in @(Get-GithubMirrorList)) {
        if ($baseUrl -eq $selectedBase) {
            continue
        }
        $version = Get-LatestVersionViaRedirect -BaseUrl $baseUrl
        if ($version) {
            return $version
        }
    }

    Write-Warning "Redirect-based version detection failed on all mirrors, falling back to GitHub API..."

    $version = Get-LatestVersionViaApi
    if ($version) {
        return $version
    }

    Write-Error "Failed to detect latest version"
    Write-Host ""
    Write-Host "Solutions:"
    Write-Host "  1. Specify a version manually:"
    Write-Host "     .\install-binary.ps1 -Version v0.2.0"
    Write-Host "  2. Download directly from:"
    Write-Host "     https://github.com/$REPO/releases"
    exit 1
}

function Get-FileHash256 {
    param([string]$FilePath)
    $hash = Get-FileHash -Path $FilePath -Algorithm SHA256
    return $hash.Hash.ToLower()
}

function Join-Path3 {
    param(
        [string]$Path,
        [string]$ChildPath,
        [string]$GrandchildPath
    )

    return (Join-Path (Join-Path $Path $ChildPath) $GrandchildPath)
}

function Ensure-SystemNetHttp {
    if ("System.Net.Http.HttpClient" -as [type]) {
        return
    }

    Add-Type -AssemblyName System.Net.Http -ErrorAction Stop

    if (-not ("System.Net.Http.HttpClient" -as [type])) {
        throw "System.Net.Http.HttpClient is unavailable"
    }
}

function Invoke-BifrostDownload {
    param(
        [string]$Uri,
        [string]$OutFile
    )

    Ensure-SystemNetHttp

    $timeout = Get-IntEnv -Name "BIFROST_DOWNLOAD_TIMEOUT" -Default 120
    $tries = Get-IntEnv -Name "BIFROST_DOWNLOAD_TRIES" -Default 2
    $lastError = $null

    for ($attempt = 1; $attempt -le $tries; $attempt++) {
        try {
            $handler = [System.Net.Http.HttpClientHandler]::new()
            $handler.AllowAutoRedirect = $true
            $client = [System.Net.Http.HttpClient]::new($handler)
            $client.Timeout = [TimeSpan]::FromSeconds($timeout)
            try {
                $response = $client.GetAsync($Uri, [System.Net.Http.HttpCompletionOption]::ResponseHeadersRead).GetAwaiter().GetResult()
                $response.EnsureSuccessStatusCode() | Out-Null

                $total = $response.Content.Headers.ContentLength
                $inputStream = $response.Content.ReadAsStreamAsync().GetAwaiter().GetResult()
                $outputStream = [System.IO.File]::Open($OutFile, [System.IO.FileMode]::Create, [System.IO.FileAccess]::Write, [System.IO.FileShare]::None)
                try {
                    $buffer = New-Object byte[] (64 * 1024)
                    $downloaded = [int64]0
                    while (($read = $inputStream.Read($buffer, 0, $buffer.Length)) -gt 0) {
                        $outputStream.Write($buffer, 0, $read)
                        $downloaded += $read
                        if ($total -and $total -gt 0) {
                            $percent = [Math]::Min(100, [Math]::Round(($downloaded * 100.0) / $total, 1))
                            Write-Progress -Activity "Downloading Bifrost" -Status "$percent% ($downloaded/$total bytes)" -PercentComplete $percent
                        }
                        else {
                            Write-Progress -Activity "Downloading Bifrost" -Status "$downloaded bytes downloaded"
                        }
                    }
                }
                finally {
                    $outputStream.Dispose()
                    $inputStream.Dispose()
                }
            }
            finally {
                if ($response) { $response.Dispose() }
                $client.Dispose()
                $handler.Dispose()
            }
            Write-Progress -Activity "Downloading Bifrost" -Completed
            if ((Test-Path $OutFile) -and ((Get-Item $OutFile).Length -gt 0)) {
                return $true
            }
            $lastError = "Downloaded file is empty: $OutFile"
        }
        catch {
            Write-Progress -Activity "Downloading Bifrost" -Completed
            $lastError = $_
        }

        if ($attempt -lt $tries) {
            Start-Sleep -Seconds 1
        }
    }

    if ($lastError) {
        Write-Warning "Download failed: $lastError"
    }

    return $false
}

function Download-GithubFile {
    param(
        [string]$GithubPath,
        [string]$OutFile
    )

    $selectedBase = Select-FastestGithubBase -GithubPath $GithubPath
    if ($selectedBase) {
        $label = Get-MirrorDisplayName -BaseUrl $selectedBase
        $url = Join-GithubUrl -BaseUrl $selectedBase -GithubPath $GithubPath
        Write-Step "Selected fastest available source: $label"
        Write-Step "Downloading from: $url"

        if (Invoke-BifrostDownload -Uri $url -OutFile $OutFile) {
            Write-Success "Downloaded via $label"
            return $true
        }

        Write-Warning "Selected source failed during full download, falling back to all mirrors"
    }
    else {
        Write-Warning "Could not probe GitHub mirrors, falling back to all mirrors"
    }

    foreach ($baseUrl in @(Get-GithubMirrorList)) {
        if ($baseUrl -eq $selectedBase) {
            continue
        }
        $label = Get-MirrorDisplayName -BaseUrl $baseUrl
        $url = Join-GithubUrl -BaseUrl $baseUrl -GithubPath $GithubPath
        Write-Step "Downloading from: $url"
        if (Invoke-BifrostDownload -Uri $url -OutFile $OutFile) {
            Write-Success "Downloaded via $label"
            return $true
        }
    }

    return $false
}

function Install-Bifrost {
    Write-Banner

    $arch = Get-Architecture
    Write-Step "Detecting system..."
    Write-Host "  OS:           Windows"
    Write-Host "  Architecture: $arch"

    if ($arch -eq "unknown") {
        Write-Error "Unsupported architecture"
        exit 1
    }

    $target = Get-Target -Arch $arch
    if (-not $target) {
        Write-Error "No pre-built binary available for Windows-$arch"
        Write-Warning "You can build from source instead:"
        Write-Host "  git clone https://github.com/$REPO.git"
        Write-Host "  cd bifrost && cargo build --release"
        exit 1
    }

    if (-not $Version) {
        Write-Step "Fetching latest version..."
        $Version = Get-LatestVersion
    }

    Write-Success "Installing version: $Version"
    Write-Host "  Target: $target"

    if (-not (Test-Path $InstallDir)) {
        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    }

    $tmpDir = Join-Path $env:TEMP "bifrost-install-$(Get-Random)"
    New-Item -ItemType Directory -Path $tmpDir -Force | Out-Null

    try {
        Write-Step "Installing CLI..."

        $archiveFile = "bifrost-$Version-$target.zip"
        $archivePathOnGithub = "$REPO/releases/download/$Version/$archiveFile"
        $checksumsPathOnGithub = "$REPO/releases/download/$Version/bifrost-$Version-checksums.txt"

        $archivePath = Join-Path $tmpDir $archiveFile
        $checksumsPath = Join-Path $tmpDir "checksums.txt"

        if (-not (Download-GithubFile -GithubPath $archivePathOnGithub -OutFile $archivePath)) {
            Write-Error "Failed to download binary"
            exit 1
        }

        Write-Step "Downloading checksums..."
        if (-not (Download-GithubFile -GithubPath $checksumsPathOnGithub -OutFile $checksumsPath)) {
            Write-Warning "Failed to download checksums, skipping verification"
            $checksumsPath = $null
        }

        if ($checksumsPath -and (Test-Path $checksumsPath)) {
            $checksumContent = Get-Content $checksumsPath
            $expectedChecksum = ($checksumContent | Where-Object { $_ -match $archiveFile } | ForEach-Object { ($_ -split '\s+')[0] })
            
            if ($expectedChecksum) {
                $actualChecksum = Get-FileHash256 -FilePath $archivePath
                if ($actualChecksum -ne $expectedChecksum.ToLower()) {
                    Write-Error "Checksum verification failed!"
                    Write-Error "Expected: $expectedChecksum"
                    Write-Error "Actual:   $actualChecksum"
                    exit 1
                }
                Write-Success "Checksum verified"
            }
            else {
                Write-Warning "Checksum not found for $archiveFile, skipping verification"
            }
        }

        Write-Step "Extracting..."
        $extractDir = Join-Path $tmpDir "extracted"
        Expand-Archive -Path $archivePath -DestinationPath $extractDir -Force

        $binaryName = "$BINARY_NAME.exe"
        $extractedDir = "bifrost-$Version-$target"
        $sourcePath = Join-Path3 -Path $extractDir -ChildPath $extractedDir -GrandchildPath $binaryName

        if (-not (Test-Path $sourcePath)) {
            $sourcePath = Join-Path $extractDir $binaryName
        }

        if (-not (Test-Path $sourcePath)) {
            $foundBinary = Get-ChildItem -Path $extractDir -Filter $binaryName -Recurse | Select-Object -First 1
            if ($foundBinary) {
                $sourcePath = $foundBinary.FullName
            }
            else {
                Write-Error "Binary not found in archive"
                exit 1
            }
        }

        $destPath = Join-Path $InstallDir $binaryName
        Install-BinaryAtomically -SourcePath $sourcePath -DestPath $destPath

        Write-Success "CLI installed: $destPath"

        $pathResult = Add-BifrostToUserPath -Directory $InstallDir
        if ($pathResult -eq "added") {
            Write-Success "Added to Windows User PATH: $InstallDir"
            Write-Success "Updated current PowerShell PATH for this session"
        }
        else {
            Write-Success "Windows User PATH already contains: $InstallDir"
            if (Test-PathListContains -PathList $env:Path -Directory $InstallDir) {
                Write-Success "Current PowerShell PATH contains: $InstallDir"
            }
            else {
                Write-Warning "Restart PowerShell/CMD to use bifrost from PATH"
            }
        }

        Install-BifrostDesktop -BinaryPath $destPath -TargetVersion $Version

        Write-Host ""
        Write-Host "------------------------------------------------------------"
        Write-Success "Installation completed!"
        Write-Host "------------------------------------------------------------"
        Write-Host ""

        Write-Host ""
        Write-Host "Getting started:"
        Write-Host ""
        Write-Host "  # Start proxy server"
        Write-Host "  bifrost start"
        Write-Host ""
        Write-Host "  # Start with custom port"
        Write-Host "  bifrost -p 8080 start"
        Write-Host ""
        Write-Host "  # Show help"
        Write-Host "  bifrost --help"
        Write-Host ""
        Write-Host "Documentation: https://github.com/$REPO"
        Write-Host ""
    }
    finally {
        if (Test-Path $tmpDir) {
            Remove-Item -Path $tmpDir -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}

if ($env:BIFROST_INSTALL_BINARY_SKIP_MAIN -ne "1") {
    Install-Bifrost
}
