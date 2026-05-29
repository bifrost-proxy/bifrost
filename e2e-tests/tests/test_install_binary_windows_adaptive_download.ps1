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
    function Test-GithubUrl {
        param([string]$Url)
        return $Url.StartsWith("https://ghfast.top/")
    }

    $selected = Select-FastestGithubBase -GithubPath "$REPO/releases/latest"
    Assert-Eq $selected "https://ghfast.top/https://github.com" "fast mirror wins when github.com probe fails"
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

Remove-Item Env:BIFROST_INSTALL_BINARY_SKIP_MAIN -ErrorAction SilentlyContinue

Write-Host ""
Write-Host "Passed: $Passed"
