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

New-Item -ItemType Directory -Path $dir -Force | Out-Null

$url = "$base/$app-windows-$arch.exe"
$tmp = Join-Path ([IO.Path]::GetTempPath()) "$app-$PID.exe"
$dest = Join-Path $dir "$app.exe"

try {
    try {
        [Net.ServicePointManager]::SecurityProtocol =
            [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    } catch {}

    Write-Host "Downloading $app (windows-$arch)..."
    Invoke-WebRequest -Uri $url -OutFile $tmp -UseBasicParsing
    if ((Get-Item -LiteralPath $tmp).Length -le 0) { throw "Download produced an empty file" }

    & $tmp --version *> $null
    if ($LASTEXITCODE -ne 0) { throw "Downloaded file did not run" }

    Move-Item -LiteralPath $tmp -Destination $dest -Force

    & $dest --version *> $null
    if ($LASTEXITCODE -ne 0) { throw "Installed file did not run" }

    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if (($userPath -split ";") -notcontains $dir) {
        $newPath = if ([string]::IsNullOrWhiteSpace($userPath)) { $dir } else { "$dir;$userPath" }
        [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    }
    if (($env:Path -split ";") -notcontains $dir) {
        $env:Path = "$dir;$env:Path"
    }

    Write-Host "Installed to $dest"
    Write-Host "Run 'cokacmux' to start."
    Write-Host "Open a new PowerShell window if the cokacmux command is not found in this one."
} catch {
    Write-Host "ERROR $($_.Exception.Message)" -ForegroundColor Red
    exit 1
} finally {
    Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
}
