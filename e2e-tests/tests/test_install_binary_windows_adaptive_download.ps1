#!/usr/bin/env pwsh

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectDir = Resolve-Path (Join-Path $ScriptDir "../..")
$Passed = 0

function Pass {
    param([string]$Message)
    Write-Host "  PASS $Message"
    $script:Passed += 1
}

function Fail {
    param(
        [string]$Message,
        [string]$Detail
    )
    Write-Host "  FAIL $Message"
    Write-Host "    $Detail"
    exit 1
}

function Assert-Eq {
    param(
        [string]$Actual,
        [string]$Expected,
        [string]$Message
    )
    if ($Actual -ne $Expected) {
        Fail $Message "expected '$Expected', got '$Actual'"
    }
    Pass $Message
}

function Run-Case {
    param(
        [string]$Message,
        [scriptblock]$Body
    )

    & $Body
    Pass $Message
}

Write-Host "==> install-binary.ps1 adaptive download source selection"

$env:BIFROST_INSTALL_BINARY_SKIP_MAIN = "1"
. (Join-Path $ProjectDir "install-binary.ps1")

Run-Case "preferred mirror ordering" {
    $env:BIFROST_GITHUB_MIRROR = "https://ghfast.top/https://github.com"
    $mirrors = @(Get-GithubMirrorList)
    Assert-Eq $mirrors[0] "https://ghfast.top/https://github.com" "BIFROST_GITHUB_MIRROR stays first"
    Assert-Eq ([string](($mirrors | Where-Object { $_ -eq "https://ghfast.top/https://github.com" }).Count)) "1" "preferred mirror is not duplicated"
    Remove-Item Env:BIFROST_GITHUB_MIRROR -ErrorAction SilentlyContinue
}

Run-Case "fastest mirror probe selection" {
    $env:BIFROST_INSTALLER_TEST_DISABLE_JOBS = "1"
    function Test-GithubUrl {
        param([string]$Url)
        return $Url.StartsWith("https://ghfast.top/")
    }

    $selected = Select-FastestGithubBase -GithubPath "$REPO/releases/latest"
    Assert-Eq $selected "https://ghfast.top/https://github.com" "fast mirror wins when github.com probe fails"
    Remove-Item Env:BIFROST_INSTALLER_TEST_DISABLE_JOBS -ErrorAction SilentlyContinue
}

Run-Case "latest version redirect selection" {
    function Select-FastestGithubBase {
        param([string]$GithubPath)
        return "https://ghfast.top/https://github.com"
    }
    function Get-LatestVersionViaRedirect {
        param([string]$BaseUrl)
        if ($BaseUrl -eq "https://ghfast.top/https://github.com") {
            return "v9.8.7"
        }
        return $null
    }
    function Get-LatestVersionViaApi {
        return $null
    }

    $version = Get-LatestVersion
    Assert-Eq $version "v9.8.7" "latest version detection accepts selected mirror redirect"
}

Run-Case "selected source full download" {
    $tmpDir = New-Item -ItemType Directory -Path (Join-Path ([System.IO.Path]::GetTempPath()) "bifrost-ps-test-$([guid]::NewGuid())")
    try {
        function Select-FastestGithubBase {
            param([string]$GithubPath)
            return "https://ghfast.top/https://github.com"
        }
        function Invoke-BifrostDownload {
            param(
                [string]$Uri,
                [string]$OutFile
            )
            Set-Content -Path (Join-Path $tmpDir "selected-url.txt") -Value $Uri
            Set-Content -Path $OutFile -Value "archive"
            return $true
        }

        $outFile = Join-Path $tmpDir "out.zip"
        Download-GithubFile -GithubPath "$REPO/releases/download/v1.0.0/bifrost-v1.0.0-test.zip" -OutFile $outFile | Out-Null
        $selectedUrl = Get-Content (Join-Path $tmpDir "selected-url.txt") -Raw
        Assert-Eq $selectedUrl.Trim() "https://ghfast.top/https://github.com/$REPO/releases/download/v1.0.0/bifrost-v1.0.0-test.zip" "full download starts with selected fastest mirror"
    }
    finally {
        Remove-Item -Path $tmpDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Run-Case "fallback full mirror list" {
    $tmpDir = New-Item -ItemType Directory -Path (Join-Path ([System.IO.Path]::GetTempPath()) "bifrost-ps-test-$([guid]::NewGuid())")
    try {
        function Select-FastestGithubBase {
            param([string]$GithubPath)
            return "https://ghfast.top/https://github.com"
        }

        $script:downloadAttempts = @()
        function Invoke-BifrostDownload {
            param(
                [string]$Uri,
                [string]$OutFile
            )
            $script:downloadAttempts += $Uri
            if ($Uri.StartsWith("https://github.com/")) {
                Set-Content -Path $OutFile -Value "archive"
                return $true
            }
            return $false
        }

        $outFile = Join-Path $tmpDir "out.zip"
        Download-GithubFile -GithubPath "$REPO/releases/download/v1.0.0/bifrost-v1.0.0-test.zip" -OutFile $outFile | Out-Null

        Assert-Eq $script:downloadAttempts[0] "https://ghfast.top/https://github.com/$REPO/releases/download/v1.0.0/bifrost-v1.0.0-test.zip" "selected mirror is attempted first"
        Assert-Eq $script:downloadAttempts[1] "https://github.com/$REPO/releases/download/v1.0.0/bifrost-v1.0.0-test.zip" "fallback tries github.com after selected mirror fails"
    }
    finally {
        Remove-Item -Path $tmpDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Run-Case "download timeout env" {
    $env:BIFROST_DOWNLOAD_TIMEOUT = "45"
    $env:BIFROST_DOWNLOAD_TRIES = "1"
    Assert-Eq ([string](Get-IntEnv -Name "BIFROST_DOWNLOAD_TIMEOUT" -Default 120)) "45" "BIFROST_DOWNLOAD_TIMEOUT is parsed"
    Assert-Eq ([string](Get-IntEnv -Name "BIFROST_DOWNLOAD_TRIES" -Default 2)) "1" "BIFROST_DOWNLOAD_TRIES is parsed"
    Remove-Item Env:BIFROST_DOWNLOAD_TIMEOUT -ErrorAction SilentlyContinue
    Remove-Item Env:BIFROST_DOWNLOAD_TRIES -ErrorAction SilentlyContinue
}

Run-Case "System.Net.Http is loaded for Windows PowerShell downloads" {
    Ensure-SystemNetHttp
    Assert-Eq ([string](("System.Net.Http.HttpClient" -as [type]) -ne $null)) "True" "HttpClient type is available after Ensure-SystemNetHttp"
}

Run-Case "archive binary path helper is compatible with Windows PowerShell 5.1" {
    $joined = Join-Path3 -Path "C:\Temp\extract" -ChildPath "bifrost-v1-target" -GrandchildPath "bifrost.exe"
    Assert-Eq $joined "C:\Temp\extract\bifrost-v1-target\bifrost.exe" "Join-Path3 builds nested archive binary path"
}

Run-Case "Windows User PATH helper is case-insensitive and slash-tolerant" {
    $pathList = "C:\Tools;C:\Users\eden_studio\.local\bin\;C:\Other"
    $contains = Test-PathListContains -PathList $pathList -Directory "c:\users\EDEN_STUDIO\.local\bin"
    Assert-Eq ([string]$contains) "True" "PATH helper detects existing entry regardless of case and trailing slash"
}

Run-Case "Windows User PATH helper prepends and deduplicates" {
    $pathList = "C:\Tools;C:\Other"
    $updated = Add-PathListEntry -PathList $pathList -Directory "C:\Users\eden_studio\.local\bin"
    Assert-Eq $updated "C:\Users\eden_studio\.local\bin;C:\Tools;C:\Other" "PATH helper prepends missing install dir"

    $again = Add-PathListEntry -PathList "C:\Tools;c:\users\EDEN_STUDIO\.local\bin\;C:\Other" -Directory "C:\Users\eden_studio\.local\bin"
    Assert-Eq $again $updated "PATH helper does not duplicate existing install dir"
}

Run-Case "architecture helper maps RuntimeInformation values" {
    Assert-Eq (Resolve-BifrostArchitecture -RuntimeArchitecture "X64" -NativeArchitecture "" -ProcessArchitecture "" -Is64BitOperatingSystem $true) "x86_64" "RuntimeInformation X64 maps to x86_64"
    Assert-Eq (Resolve-BifrostArchitecture -RuntimeArchitecture "Arm64" -NativeArchitecture "" -ProcessArchitecture "" -Is64BitOperatingSystem $true) "aarch64" "RuntimeInformation Arm64 maps to aarch64"
}

Run-Case "architecture helper falls back to Windows environment variables" {
    Assert-Eq (Resolve-BifrostArchitecture -RuntimeArchitecture "unknown" -NativeArchitecture "ARM64" -ProcessArchitecture "x86" -Is64BitOperatingSystem $true) "aarch64" "PROCESSOR_ARCHITEW6432 ARM64 wins for 32-bit PowerShell on ARM64 Windows"
    Assert-Eq (Resolve-BifrostArchitecture -RuntimeArchitecture $null -NativeArchitecture "" -ProcessArchitecture "AMD64" -Is64BitOperatingSystem $true) "x86_64" "PROCESSOR_ARCHITECTURE AMD64 maps to x86_64"
    Assert-Eq (Resolve-BifrostArchitecture -RuntimeArchitecture $null -NativeArchitecture "" -ProcessArchitecture "" -Is64BitOperatingSystem $true) "x86_64" "64-bit OS fallback maps to x86_64"
}

Remove-Item Env:BIFROST_INSTALL_BINARY_SKIP_MAIN -ErrorAction SilentlyContinue

Write-Host ""
Write-Host "Passed: $Passed"
