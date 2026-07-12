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

function Install-BinaryAtomically {
    param(
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Destination
    )

    $targetDir = Split-Path -Parent $Destination
    $targetName = Split-Path -Leaf $Destination
    $staged = Join-Path $targetDir ".$targetName-$PID-$([IO.Path]::GetRandomFileName()).tmp"
    try {
        Copy-Item -LiteralPath $Source -Destination $staged
        Move-Item -LiteralPath $staged -Destination $Destination -Force
    } finally {
        Remove-Item -LiteralPath $staged -Force -ErrorAction SilentlyContinue
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

try {
    try {
        [Net.ServicePointManager]::SecurityProtocol =
            [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    } catch {}

    Download-Binary -Name "$app (windows-$arch)" -Uri $url -OutFile $tmp
    Download-Binary -Name "$cokacdirApp (windows-$arch)" -Uri $cokacdirUrl -OutFile $cokacdirTmp

    Assert-BinaryVersion -Name $app -Path $tmp
    Assert-BinaryVersion -Name $cokacdirApp -Path $cokacdirTmp

    Install-BinaryAtomically -Source $cokacdirTmp -Destination $cokacdirDest
    Install-BinaryAtomically -Source $tmp -Destination $dest

    Assert-BinaryVersion -Name $app -Path $dest
    Assert-BinaryVersion -Name $cokacdirApp -Path $cokacdirDest

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
