#!/usr/bin/env pwsh
#Requires -Version 5.1

<#
.SYNOPSIS
    Bifrost installation script for Windows
.DESCRIPTION
    Downloads and installs the Bifrost proxy server binary
.PARAMETER Version
    Specific version to install (e.g., v0.1.0). If not specified, installs the latest version.
.PARAMETER InstallDir
    Installation directory. Defaults to $env:LOCALAPPDATA\bifrost\bin
.EXAMPLE
    irm https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.ps1 | iex
.EXAMPLE
    .\install-binary.ps1 -Version v0.0.9-alpha
.EXAMPLE
    .\install-binary.ps1 -InstallDir "C:\Tools\bifrost"
#>

param(
    [string]$Version = "",
    [string]$InstallDir = ""
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
    Write-Host "╔═══════════════════════════════════════════════════════════╗" -ForegroundColor Cyan
    Write-Host "║                                                           ║" -ForegroundColor Cyan
    Write-Host "║   ____  _  __                _                            ║" -ForegroundColor Cyan
    Write-Host "║  |  _ \(_)/ _|_ __ ___  ___| |_                           ║" -ForegroundColor Cyan
    Write-Host "║  | |_) | | |_| '__/ _ \/ __| __|                          ║" -ForegroundColor Cyan
    Write-Host "║  |  _ <| |  _| | | (_) \__ \ |_                           ║" -ForegroundColor Cyan
    Write-Host "║  |_| \_\_|_| |_|  \___/|___/\__|                          ║" -ForegroundColor Cyan
    Write-Host "║                                                           ║" -ForegroundColor Cyan
    Write-Host "║   High-performance HTTP/HTTPS/SOCKS5 Proxy Server         ║" -ForegroundColor Cyan
    Write-Host "║                                                           ║" -ForegroundColor Cyan
    Write-Host "╚═══════════════════════════════════════════════════════════╝" -ForegroundColor Cyan
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

function Get-Architecture {
    $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    switch ($arch) {
        "X64" { return "x86_64" }
        "Arm64" { return "aarch64" }
        default { return "unknown" }
    }
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

function Invoke-BifrostDownload {
    param(
        [string]$Uri,
        [string]$OutFile
    )

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
        $sourcePath = Join-Path $extractDir $extractedDir $binaryName

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
        Copy-Item -Path $sourcePath -Destination $destPath -Force

        Write-Success "CLI installed: $destPath"

        Write-Host ""
        Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        Write-Success "Installation completed!"
        Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        Write-Host ""

        $currentPath = [Environment]::GetEnvironmentVariable("Path", "User")
        if ($currentPath -notlike "*$InstallDir*") {
            Write-Warning "$InstallDir is not in your PATH"
            Write-Host ""
            Write-Host "To add it permanently, run:"
            Write-Host ""
            Write-Host "  `$currentPath = [Environment]::GetEnvironmentVariable('Path', 'User')" -ForegroundColor Gray
            Write-Host "  [Environment]::SetEnvironmentVariable('Path', `"`$currentPath;$InstallDir`", 'User')" -ForegroundColor Gray
            Write-Host ""
            Write-Host "Or add it temporarily for this session:"
            Write-Host ""
            Write-Host "  `$env:Path += `";$InstallDir`"" -ForegroundColor Gray
        }

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
