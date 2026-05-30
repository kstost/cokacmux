#
# COKACMUX installer and updater for Windows
# Usage: irm https://raw.githubusercontent.com/kstost/cokacmux/refs/heads/main/manage.ps1 | iex
#

param(
    [Parameter(Position = 0)]
    [string]$Command = "install"
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$BINARY_NAME = "cokacmux"
$DEFAULT_BASE_URL = "https://raw.githubusercontent.com/kstost/cokacmux/refs/heads/main/dist_beta"
$BASE_URL = if ([string]::IsNullOrWhiteSpace($env:COKACMUX_BASE_URL)) {
    $DEFAULT_BASE_URL
} else {
    $env:COKACMUX_BASE_URL.TrimEnd("/")
}

function Info($msg) { Write-Host "-> $msg" -ForegroundColor Blue }
function Success($msg) { Write-Host "OK $msg" -ForegroundColor Green }
function Warn($msg) { Write-Host "! $msg" -ForegroundColor Yellow }
function Fail($msg) { throw $msg }

function Show-Usage {
    Write-Host @"
cokacmux installer

Usage:
  manage.ps1 [install|update]
  manage.ps1 uninstall
  manage.ps1 --help

Examples:
  irm https://cokacmux.cokac.com/manage.ps1 | iex
  iex "& { `$(irm https://cokacmux.cokac.com/manage.ps1) } uninstall"
  `$env:COKACMUX_INSTALL_DIR = "`$env:USERPROFILE\bin"; .\manage.ps1

Environment:
  COKACMUX_INSTALL_DIR       Install directory override
  COKACMUX_BASE_URL          Download base URL override
  COKACMUX_REQUIRE_CHECKSUM  Set to 1 to fail when .sha256 is unavailable
"@
}

function Enable-Tls12 {
    try {
        [Net.ServicePointManager]::SecurityProtocol =
            [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    } catch {
        # PowerShell 7+ on modern Windows does not need this fallback.
    }
}

function Detect-Arch {
    $arch = if (-not [string]::IsNullOrWhiteSpace($env:PROCESSOR_ARCHITEW6432)) {
        $env:PROCESSOR_ARCHITEW6432
    } else {
        $env:PROCESSOR_ARCHITECTURE
    }

    switch ($arch.ToUpperInvariant()) {
        "AMD64" { return "x86_64" }
        "ARM64" { return "aarch64" }
        default { Fail "Unsupported architecture: $arch" }
    }
}

function Get-InstallDir([bool]$Create = $true) {
    if (-not [string]::IsNullOrWhiteSpace($env:COKACMUX_INSTALL_DIR)) {
        $dir = [Environment]::ExpandEnvironmentVariables($env:COKACMUX_INSTALL_DIR)
    } elseif (-not [string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
        $dir = Join-Path $env:LOCALAPPDATA "cokacmux"
    } else {
        Fail "LOCALAPPDATA is not set. Set COKACMUX_INSTALL_DIR and try again."
    }

    if ($Create) {
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
    }
    return $dir
}

function Normalize-PathForCompare($path) {
    try {
        $expanded = [Environment]::ExpandEnvironmentVariables($path)
        return [IO.Path]::GetFullPath($expanded).TrimEnd([char[]]@("\", "/"))
    } catch {
        return $path.Trim().TrimEnd([char[]]@("\", "/"))
    }
}

function Path-ContainsEntry($pathValue, $dir) {
    if ([string]::IsNullOrWhiteSpace($pathValue)) {
        return $false
    }

    $target = Normalize-PathForCompare $dir
    foreach ($entry in ($pathValue -split ";")) {
        if ([string]::IsNullOrWhiteSpace($entry)) {
            continue
        }
        $normalized = Normalize-PathForCompare $entry
        if ([StringComparer]::OrdinalIgnoreCase.Equals($normalized, $target)) {
            return $true
        }
    }
    return $false
}

function Add-ToPath($dir) {
    $currentPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($null -eq $currentPath) {
        $currentPath = ""
    }

    if (-not (Path-ContainsEntry $currentPath $dir)) {
        $newPath = if ([string]::IsNullOrWhiteSpace($currentPath)) { $dir } else { "$dir;$currentPath" }
        [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
        Success "Added $dir to user PATH."
    }

    if (-not (Path-ContainsEntry $env:Path $dir)) {
        $env:Path = "$dir;$env:Path"
    }
}

function Remove-FromPath($dir) {
    $currentPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ([string]::IsNullOrWhiteSpace($currentPath)) {
        return
    }

    $target = Normalize-PathForCompare $dir
    $kept = @()
    foreach ($entry in ($currentPath -split ";")) {
        if ([string]::IsNullOrWhiteSpace($entry)) {
            continue
        }
        $normalized = Normalize-PathForCompare $entry
        if (-not ([StringComparer]::OrdinalIgnoreCase.Equals($normalized, $target))) {
            $kept += $entry
        }
    }

    $newPath = ($kept -join ";")
    if ($newPath -ne $currentPath) {
        [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
        $processKept = @()
        foreach ($entry in ($env:Path -split ";")) {
            if ([string]::IsNullOrWhiteSpace($entry)) {
                continue
            }
            $normalized = Normalize-PathForCompare $entry
            if (-not ([StringComparer]::OrdinalIgnoreCase.Equals($normalized, $target))) {
                $processKept += $entry
            }
        }
        $env:Path = ($processKept -join ";")
        Success "Removed $dir from user PATH."
    }
}

function Download-File($url, $outFile) {
    Enable-Tls12

    if ($url -match "^file://") {
        $uri = [Uri]$url
        Copy-Item -LiteralPath $uri.LocalPath -Destination $outFile -Force
        return
    }

    Invoke-WebRequest -Uri $url -OutFile $outFile -UseBasicParsing
}

function Verify-Checksum($url, $file) {
    $checksumUrl = "$url.sha256"
    $checksumFile = [IO.Path]::GetTempFileName()

    try {
        try {
            Download-File $checksumUrl $checksumFile
        } catch {
            if ($env:COKACMUX_REQUIRE_CHECKSUM -eq "1") {
                Fail "Checksum file is unavailable: $checksumUrl"
            }
            Warn "Checksum file is unavailable; continuing without checksum verification."
            return
        }

        $content = (Get-Content -LiteralPath $checksumFile -Raw).Trim()
        $expected = ($content -split "\s+")[0].ToLowerInvariant()
        if ($expected -notmatch "^[0-9a-f]{64}$") {
            Fail "Checksum file is invalid: $checksumUrl"
        }

        if (-not (Get-Command Get-FileHash -ErrorAction SilentlyContinue)) {
            if ($env:COKACMUX_REQUIRE_CHECKSUM -eq "1") {
                Fail "Get-FileHash is required for checksum verification"
            }
            Warn "Get-FileHash is unavailable; continuing without checksum verification."
            return
        }

        $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $file).Hash.ToLowerInvariant()
        if ($expected -ne $actual) {
            Fail "Checksum mismatch for downloaded binary"
        }

        Success "Checksum verified."
    } finally {
        Remove-Item -LiteralPath $checksumFile -Force -ErrorAction SilentlyContinue
    }
}

function Stop-InstalledProcess($installPath) {
    if (-not (Test-Path -LiteralPath $installPath)) {
        return
    }

    $target = Normalize-PathForCompare $installPath
    $stopped = 0

    foreach ($process in (Get-Process -Name $BINARY_NAME -ErrorAction SilentlyContinue)) {
        try {
            if ([string]::IsNullOrWhiteSpace($process.Path)) {
                continue
            }
            $processPath = Normalize-PathForCompare $process.Path
        } catch {
            continue
        }

        if ([StringComparer]::OrdinalIgnoreCase.Equals($processPath, $target)) {
            Warn "Stopping running cokacmux process $($process.Id) so the executable can be replaced."
            Stop-Process -Id $process.Id -Force -ErrorAction Stop
            $stopped += 1
        }
    }

    if ($stopped -gt 0) {
        Start-Sleep -Seconds 1
    }
}

function Install-Binary($downloadedFile, $installPath) {
    $installDir = Split-Path -Parent $installPath
    New-Item -ItemType Directory -Path $installDir -Force | Out-Null

    $stagedPath = Join-Path $installDir "$BINARY_NAME.exe.new"
    Remove-Item -LiteralPath $stagedPath -Force -ErrorAction SilentlyContinue
    Copy-Item -LiteralPath $downloadedFile -Destination $stagedPath -Force

    try {
        if (Test-Path -LiteralPath $installPath) {
            [IO.File]::Replace($stagedPath, $installPath, $null, $true)
        } else {
            Move-Item -LiteralPath $stagedPath -Destination $installPath -Force
        }
    } catch {
        Remove-Item -LiteralPath $stagedPath -Force -ErrorAction SilentlyContinue
        Fail "Could not replace $installPath. Close any running cokacmux window and try again. $_"
    }
}

function Verify-Installed($installPath) {
    if (-not (Test-Path -LiteralPath $installPath)) {
        Fail "Installation failed: $installPath was not created"
    }

    try {
        $output = & $installPath --version 2>&1
        if ($LASTEXITCODE -ne 0) {
            Fail "Installation failed: '$installPath --version' exited with code $LASTEXITCODE. $output"
        }
    } catch {
        Fail "Installation failed: '$installPath --version' did not run successfully. $_"
    }
}

function Install-Main {
    $arch = Detect-Arch
    $filename = "${BINARY_NAME}-windows-${arch}.exe"
    $url = "$BASE_URL/$filename"
    $installDir = Get-InstallDir
    $installPath = Join-Path $installDir "${BINARY_NAME}.exe"
    $tmpFile = [IO.Path]::GetTempFileName()

    try {
        Info "Downloading cokacmux (windows-$arch)..."
        Download-File $url $tmpFile

        if ((Get-Item -LiteralPath $tmpFile).Length -le 0) {
            Fail "Download produced an empty file: $url"
        }

        Verify-Checksum $url $tmpFile
        Stop-InstalledProcess $installPath
        Install-Binary $tmpFile $installPath
        Verify-Installed $installPath
        Add-ToPath $installDir

        Success "Installed to $installPath"
        Success "Run 'cokacmux' to start."
        Warn "Open a new PowerShell window if the cokacmux command is not found in this one."
    } finally {
        Remove-Item -LiteralPath $tmpFile -Force -ErrorAction SilentlyContinue
    }
}

function Uninstall-Main {
    $installDir = Get-InstallDir $false
    $installPath = Join-Path $installDir "${BINARY_NAME}.exe"

    Stop-InstalledProcess $installPath

    if (Test-Path -LiteralPath $installPath) {
        Remove-Item -LiteralPath $installPath -Force
        Success "Removed $installPath"
    } else {
        Warn "No cokacmux executable found at $installPath."
    }

    Remove-FromPath $installDir

    try {
        Remove-Item -LiteralPath $installDir -Force -ErrorAction Stop
    } catch {
        # Keep the directory if it still contains user-created files.
    }

    $settingsDir = Join-Path $env:USERPROFILE ".cokacmux"
    Warn "Settings and session data under $settingsDir were not removed."
}

function Main {
    switch ($Command.ToLowerInvariant()) {
        "install" { Install-Main }
        "update" { Install-Main }
        "uninstall" { Uninstall-Main }
        "remove" { Uninstall-Main }
        "help" { Show-Usage }
        "-h" { Show-Usage }
        "--help" { Show-Usage }
        default {
            Show-Usage
            Fail "Unknown command: $Command"
        }
    }
}

try {
    Main
} catch {
    Write-Host "ERROR $($_.Exception.Message)" -ForegroundColor Red
    exit 1
}
