# Usage: irm https://cokacmux.cokac.com/manage.ps1 | iex

param([Parameter(Position = 0)][string]$Command = "install")

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$app = "cokacmux"
$base = if ($env:COKACMUX_BASE_URL) {
    $env:COKACMUX_BASE_URL.TrimEnd("/")
} else {
    "https://raw.githubusercontent.com/kstost/cokacmux/refs/heads/main/dist_beta"
}
$cokacdirApp = "cokacdir"
$cokacdirBase = if ($env:COKACDIR_BASE_URL) {
    $env:COKACDIR_BASE_URL.TrimEnd("/")
} else {
    "https://raw.githubusercontent.com/kstost/cokacdir/main/dist"
}

if ($Command -in @("help", "-h", "--help")) {
    Write-Host "Usage: manage.ps1 [install|update]"
    exit 0
}
if ($Command -notin @("install", "update")) {
    throw "Only install/update is supported by this installer."
}

$arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
switch ($arch.ToUpperInvariant()) {
    "AMD64" { $arch = "x86_64" }
    "ARM64" { $arch = "aarch64" }
    default { throw "Unsupported architecture: $arch" }
}

$dir = if ($env:COKACMUX_INSTALL_DIR) {
    [Environment]::ExpandEnvironmentVariables($env:COKACMUX_INSTALL_DIR)
} elseif ($env:LOCALAPPDATA) {
    Join-Path $env:LOCALAPPDATA $app
} else {
    throw "LOCALAPPDATA is not set. Set COKACMUX_INSTALL_DIR and try again."
}
$homeDir = [Environment]::GetFolderPath("UserProfile")
if ([string]::IsNullOrWhiteSpace($homeDir)) {
    $homeDir = $env:USERPROFILE
}
if ([string]::IsNullOrWhiteSpace($homeDir)) {
    throw "USERPROFILE is not set. Cannot choose cokacdir install directory."
}
$cokacdirDir = Join-Path (Join-Path $homeDir ".cokacmux") "bin"

New-Item -ItemType Directory -Path $dir -Force | Out-Null
New-Item -ItemType Directory -Path $cokacdirDir -Force | Out-Null

$url = "$base/$app-windows-$arch.exe"
$cokacdirUrl = "$cokacdirBase/$cokacdirApp-windows-$arch.exe"
$tmp = Join-Path ([IO.Path]::GetTempPath()) "$app-$PID-$([IO.Path]::GetRandomFileName()).exe"
$cokacdirTmp = Join-Path ([IO.Path]::GetTempPath()) "$cokacdirApp-$PID-$([IO.Path]::GetRandomFileName()).exe"
$dest = Join-Path $dir "$app.exe"
$cokacdirDest = Join-Path $cokacdirDir "$cokacdirApp.exe"

function Download-Binary {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Uri,
        [Parameter(Mandatory = $true)][string]$OutFile
    )

    Write-Host "Downloading $Name..."
    Invoke-WebRequest -Uri $Uri -OutFile $OutFile -UseBasicParsing
    if ((Get-Item -LiteralPath $OutFile).Length -le 0) {
        throw "$Name download produced an empty file"
    }
}

function Assert-BinaryVersion {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Path
    )

    $versionOutput = (& $Path --version 2>&1 | Out-String).Trim()
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        throw "$Name file did not run"
    }
    if ($versionOutput -notmatch "^$([Regex]::Escape($Name))(?:\s|$)") {
        throw "$Name file returned an unexpected version: $versionOutput"
    }
}

function Install-BinaryPairAtomically {
    param(
        [Parameter(Mandatory = $true)][string]$AppSource,
        [Parameter(Mandatory = $true)][string]$AppDestination,
        [Parameter(Mandatory = $true)][string]$HelperSource,
        [Parameter(Mandatory = $true)][string]$HelperDestination
    )

    $appDir = Split-Path -Parent $AppDestination
    $appName = Split-Path -Leaf $AppDestination
    $helperDir = Split-Path -Parent $HelperDestination
    $helperName = Split-Path -Leaf $HelperDestination
    $appStaged = Join-Path $appDir ".$appName-$PID-$([IO.Path]::GetRandomFileName()).tmp"
    $helperStaged = Join-Path $helperDir ".$helperName-$PID-$([IO.Path]::GetRandomFileName()).tmp"
    $appBackup = Join-Path $appDir ".$appName-$PID-$([IO.Path]::GetRandomFileName()).backup"
    $helperBackup = Join-Path $helperDir ".$helperName-$PID-$([IO.Path]::GetRandomFileName()).backup"
    $appExisted = Test-Path -LiteralPath $AppDestination
    $helperExisted = Test-Path -LiteralPath $HelperDestination
    $appBackupMoved = $false
    $helperBackupMoved = $false
    $appInstalled = $false
    $helperInstalled = $false
    $committed = $false

    if ($appExisted -and -not (Test-Path -LiteralPath $AppDestination -PathType Leaf)) {
        throw "App destination is not a file: $AppDestination"
    }
    if ($helperExisted -and -not (Test-Path -LiteralPath $HelperDestination -PathType Leaf)) {
        throw "Helper destination is not a file: $HelperDestination"
    }

    try {
        # Prepare both files in their destination filesystems before changing
        # either installed program.
        Copy-Item -LiteralPath $AppSource -Destination $appStaged
        Copy-Item -LiteralPath $HelperSource -Destination $helperStaged

        if ($helperExisted) {
            Move-Item -LiteralPath $HelperDestination -Destination $helperBackup
            $helperBackupMoved = $true
        }
        Move-Item -LiteralPath $helperStaged -Destination $HelperDestination
        $helperInstalled = $true

        if ($appExisted) {
            Move-Item -LiteralPath $AppDestination -Destination $appBackup
            $appBackupMoved = $true
        }
        Move-Item -LiteralPath $appStaged -Destination $AppDestination
        $appInstalled = $true

        Assert-BinaryVersion -Name $app -Path $AppDestination
        Assert-BinaryVersion -Name $cokacdirApp -Path $HelperDestination
        $committed = $true
    } catch {
        $installError = $_.Exception.Message
        $rollbackErrors = [System.Collections.Generic.List[string]]::new()

        try {
            if ($appInstalled -and (Test-Path -LiteralPath $AppDestination)) {
                Remove-Item -LiteralPath $AppDestination -Force
            }
            if ($appBackupMoved) {
                Move-Item -LiteralPath $appBackup -Destination $AppDestination -Force
                $appBackupMoved = $false
            }
        } catch {
            $rollbackErrors.Add("app rollback failed: $($_.Exception.Message)")
        }

        try {
            if ($helperInstalled -and (Test-Path -LiteralPath $HelperDestination)) {
                Remove-Item -LiteralPath $HelperDestination -Force
            }
            if ($helperBackupMoved) {
                Move-Item -LiteralPath $helperBackup -Destination $HelperDestination -Force
                $helperBackupMoved = $false
            }
        } catch {
            $rollbackErrors.Add("helper rollback failed: $($_.Exception.Message)")
        }

        $rollbackSuffix = if ($rollbackErrors.Count -gt 0) {
            "; " + ($rollbackErrors -join "; ")
        } else {
            "; previous pair restored"
        }
        throw "$installError$rollbackSuffix"
    } finally {
        Remove-Item -LiteralPath $appStaged -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $helperStaged -Force -ErrorAction SilentlyContinue
        if ($committed) {
            foreach ($backup in @($appBackup, $helperBackup)) {
                if (-not (Test-Path -LiteralPath $backup)) {
                    continue
                }
                try {
                    Remove-Item -LiteralPath $backup -Force -ErrorAction Stop
                } catch {
                    Write-Warning "Installed pair is valid, but old backup remains at ${backup}: $($_.Exception.Message)"
                }
            }
        }
    }
}

try {
    try {
        [Net.ServicePointManager]::SecurityProtocol =
            [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    } catch {}

    Download-Binary -Name "$app (windows-$arch)" -Uri $url -OutFile $tmp
    Download-Binary -Name "$cokacdirApp (windows-$arch)" -Uri $cokacdirUrl -OutFile $cokacdirTmp

    Assert-BinaryVersion -Name $app -Path $tmp
    Assert-BinaryVersion -Name $cokacdirApp -Path $cokacdirTmp

    Install-BinaryPairAtomically `
        -AppSource $tmp `
        -AppDestination $dest `
        -HelperSource $cokacdirTmp `
        -HelperDestination $cokacdirDest

    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if (($userPath -split ";") -notcontains $dir) {
        $newPath = if ([string]::IsNullOrWhiteSpace($userPath)) { $dir } else { "$dir;$userPath" }
        [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    }
    if (($env:Path -split ";") -notcontains $dir) {
        $env:Path = "$dir;$env:Path"
    }

    Write-Host "Installed $app to $dest"
    Write-Host "Installed $cokacdirApp to $cokacdirDest"
    Write-Host "Run 'cokacmux' to start."
    Write-Host "Open a new PowerShell window if the cokacmux command is not found in this one."
} catch {
    Write-Host "ERROR $($_.Exception.Message)" -ForegroundColor Red
    exit 1
} finally {
    Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $cokacdirTmp -Force -ErrorAction SilentlyContinue
}
