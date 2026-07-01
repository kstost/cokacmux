param(
    [string]$Exe = ".\dist_beta\cokacmux-windows-aarch64-gnullvm.exe"
)

$ErrorActionPreference = "Stop"
trap {
    Write-Error $_
    exit 1
}

function Fail([string]$Message) {
    throw "windows-process-smoke failed: $Message"
}

function Wait-Until {
    param(
        [string]$Description,
        [scriptblock]$Condition,
        [int]$TimeoutSeconds = 10
    )

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        if (& $Condition) {
            return
        }
        Start-Sleep -Milliseconds 100
    } while ((Get-Date) -lt $deadline)

    Fail "timed out waiting for $Description"
}

function Read-JsonFile([string]$Path) {
    Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
}

function Write-Utf8NoBom {
    param(
        [string]$Path,
        [string]$Value
    )

    $encoding = [System.Text.UTF8Encoding]::new($false)
    [System.IO.File]::WriteAllText($Path, $Value, $encoding)
}

function Invoke-Cokacmux {
    param(
        [string]$File,
        [string[]]$Arguments
    )

    $output = & $File @Arguments 2>&1
    [pscustomobject]@{
        ExitCode = $LASTEXITCODE
        Output = ($output -join "`n")
    }
}

function Assert-ProcessGone([int]$ProcessId, [string]$Label) {
    Wait-Until "$Label process $ProcessId to exit" {
        -not (Get-Process -Id $ProcessId -ErrorAction SilentlyContinue)
    } 15
}

function Stop-ProcessIfAlive([int]$ProcessId) {
    $process = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
    if ($process) {
        Stop-Process -Id $ProcessId -Force -ErrorAction SilentlyContinue
        Wait-Process -Id $ProcessId -Timeout 5 -ErrorAction SilentlyContinue
    }
}

$exePath = (Resolve-Path -LiteralPath $Exe).Path
$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
$runId = [guid]::NewGuid().ToString("N")
$root = Join-Path $repoRoot "target\windows-process-smoke\$runId"
$configDir = Join-Path $root "config"
$homeDir = Join-Path $root "home"
$cwd = Join-Path $root "project"
$toolsDir = Join-Path $root "tools"
$fakeAgent = Join-Path $toolsDir "fake-agent.ps1"
$fakeAgentLog = Join-Path $root "fake-agent.log"
$daemonOut = Join-Path $root "daemon.out"
$daemonErr = Join-Path $root "daemon.err"
$dupOut = Join-Path $root "duplicate.out"
$dupErr = Join-Path $root "duplicate.err"

$oldConfig = $env:COKACMUX_CONFIG_DIR
$oldHome = $env:COKACMUX_HOME
$oldDebug = $env:COKACMUX_DEBUG
$oldFakeLog = $env:COKACMUX_FAKE_AGENT_LOG

$daemonPid = $null
$childPid = $null
$externalPid = $null

try {
    New-Item -ItemType Directory -Force -Path $configDir, $homeDir, $cwd, $toolsDir | Out-Null

    @'
param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$Rest
)

$ErrorActionPreference = "Stop"
$line = "FAKE_AGENT_START pid=$PID cwd=$(Get-Location) args=$($Rest -join '|')"
Add-Content -LiteralPath $env:COKACMUX_FAKE_AGENT_LOG -Value $line -Encoding UTF8
[Console]::Out.WriteLine("fake-agent-ready pid=$PID")
[Console]::Out.Flush()
while ($true) {
    Start-Sleep -Milliseconds 200
}
'@ | Set-Content -LiteralPath $fakeAgent -Encoding UTF8

    $settings = @{
        cokacmux = @{
            agent_programs = @{
                codex = $fakeAgent
            }
        }
    } | ConvertTo-Json -Depth 5
    Write-Utf8NoBom (Join-Path $configDir "settings.json") $settings

    $env:COKACMUX_CONFIG_DIR = $configDir
    $env:COKACMUX_HOME = $homeDir
    $env:COKACMUX_DEBUG = "1"
    $env:COKACMUX_FAKE_AGENT_LOG = $fakeAgentLog

    $check = Invoke-Cokacmux $exePath @("--check")
    if ($check.ExitCode -ne 0 -or $check.Output -notmatch "cokacmux --check ok") {
        Fail "--check failed before daemon start: $($check.Output)"
    }

    $sessionId = "new-winprobe-$runId"
    $stem = "codex-$sessionId"
    $agentsDir = Join-Path $configDir "agents"
    $metaPath = Join-Path $agentsDir "$stem.json"
    $tcpPath = Join-Path $agentsDir "$stem.tcp"

    $daemon = Start-Process `
        -FilePath $exePath `
        -ArgumentList @("--agent-daemon", "codex", $sessionId, $cwd, "@cokacmux-new-agent", "normal") `
        -RedirectStandardOutput $daemonOut `
        -RedirectStandardError $daemonErr `
        -WindowStyle Hidden `
        -PassThru
    $daemonPid = $daemon.Id

    Wait-Until "daemon metadata" { Test-Path -LiteralPath $metaPath } 15
    Wait-Until "daemon TCP marker" { Test-Path -LiteralPath $tcpPath } 15
    Wait-Until "daemon child pid in metadata" {
        $meta = Read-JsonFile $metaPath
        $meta.pid -eq $daemonPid -and $meta.child_pid -and $meta.child_pid_start_ticks
    } 15

    $meta = Read-JsonFile $metaPath
    $childPid = [int]$meta.child_pid
    if (-not (Get-Process -Id $daemonPid -ErrorAction SilentlyContinue)) {
        Fail "daemon process $daemonPid is not alive"
    }
    if (-not (Get-Process -Id $childPid -ErrorAction SilentlyContinue)) {
        Fail "child process $childPid is not alive"
    }
    if ([int64]$meta.child_pid_start_ticks -le 0) {
        Fail "child_pid_start_ticks was not recorded"
    }

    $lockFiles = @(Get-ChildItem -LiteralPath $agentsDir -Filter "cwd-*.lock")
    if ($lockFiles.Count -ne 1) {
        Fail "expected one cwd lock, found $($lockFiles.Count)"
    }
    $lock = Read-JsonFile $lockFiles[0].FullName
    if ($lock.pid -ne $daemonPid -or -not $lock.pid_start_ticks) {
        Fail "cwd lock does not include daemon pid and pid_start_ticks"
    }

    $tcpMarker = Get-Content -LiteralPath $tcpPath -Raw
    if ($tcpMarker -notmatch "^tcp 127\.0\.0\.1:(\d+)\s*$") {
        Fail "unexpected TCP marker: $tcpMarker"
    }
    $port = [int]$Matches[1]

    $probe = [System.Net.Sockets.TcpClient]::new()
    $probe.Connect("127.0.0.1", $port)
    $probe.Close()

    $client = [System.Net.Sockets.TcpClient]::new()
    $client.ReceiveTimeout = 5000
    $client.SendTimeout = 5000
    $client.Connect("127.0.0.1", $port)
    $stream = $client.GetStream()
    $encoding = [System.Text.UTF8Encoding]::new($false)
    $writer = [System.IO.StreamWriter]::new($stream, $encoding)
    $writer.NewLine = "`n"
    $writer.AutoFlush = $true
    $reader = [System.IO.StreamReader]::new($stream, $encoding)

    $attach = @{
        type = "attach"
        cols = 100
        rows = 30
        client_pid = $PID
        client_instance_id = "windows-process-smoke"
        client_debug = $true
        client_trace = $false
    } | ConvertTo-Json -Compress
    $writer.WriteLine($attach)
    $eventLine = $reader.ReadLine()
    if (-not $eventLine) {
        Fail "attach did not return an event"
    }
    $event = $eventLine | ConvertFrom-Json
    if ($event.type -ne "attached" -or $event.daemon_pid -ne $daemonPid -or $event.child_pid -ne $childPid) {
        Fail "unexpected attach event: $eventLine"
    }
    Wait-Until "metadata attached=true" {
        $attachedMeta = Read-JsonFile $metaPath
        $attachedMeta.attached -eq $true -and $attachedMeta.attached_client_pid -eq $PID
    } 10

    $resize = @{ type = "resize"; cols = 120; rows = 32 } | ConvertTo-Json -Compress
    $writer.WriteLine($resize)
    $detach = @{ type = "detach" } | ConvertTo-Json -Compress
    $writer.WriteLine($detach)
    $client.Close()
    Wait-Until "metadata attached=false after detach" {
        $detachedMeta = Read-JsonFile $metaPath
        $detachedMeta.attached -eq $false
    } 10

    $extendedCwd = "\\?\$cwd"
    $duplicate = Start-Process `
        -FilePath $exePath `
        -ArgumentList @("--agent-daemon", "codex", "new-winprobe-dup-$runId", $extendedCwd, "@cokacmux-new-agent", "normal") `
        -RedirectStandardOutput $dupOut `
        -RedirectStandardError $dupErr `
        -WindowStyle Hidden `
        -PassThru
    if (-not $duplicate.WaitForExit(10000)) {
        Stop-ProcessIfAlive $duplicate.Id
        Fail "duplicate daemon did not exit"
    }
    if ($duplicate.ExitCode -eq 0) {
        Fail "duplicate daemon unexpectedly succeeded"
    }
    $duplicateText = ((Get-Content -LiteralPath $dupOut -Raw -ErrorAction SilentlyContinue) + "`n" + (Get-Content -LiteralPath $dupErr -Raw -ErrorAction SilentlyContinue))
    if ($duplicateText -match "\\\\\?\\") {
        Fail "duplicate conflict exposed extended path prefix: $duplicateText"
    }
    if ($duplicateText -notmatch "already uses|refusing to start another coding agent") {
        Fail "duplicate conflict did not report expected lock/conflict message: $duplicateText"
    }

    $fakeLog = Get-Content -LiteralPath $fakeAgentLog -Raw
    if ($fakeLog -notmatch "FAKE_AGENT_START" -or $fakeLog -notmatch [regex]::Escape($cwd)) {
        Fail "fake agent did not start in expected cwd: $fakeLog"
    }

    $kill = Invoke-Cokacmux $exePath @("killall")
    if ($kill.ExitCode -ne 0) {
        Fail "killall failed: $($kill.Output)"
    }
    if ($kill.Output -notmatch "killed=1") {
        Fail "killall did not report one killed daemon: $($kill.Output)"
    }
    Assert-ProcessGone $daemonPid "daemon"
    Assert-ProcessGone $childPid "child"
    if (Test-Path -LiteralPath $agentsDir) {
        Fail "agents directory still exists after killall"
    }
    if (-not (Test-Path -LiteralPath (Join-Path $configDir "settings.json"))) {
        Fail "killall removed settings.json"
    }

    New-Item -ItemType Directory -Force -Path $agentsDir | Out-Null
    $external = Start-Process `
        -FilePath "powershell.exe" `
        -ArgumentList @("-NoProfile", "-Command", "Start-Sleep -Seconds 600") `
        -WindowStyle Hidden `
        -PassThru
    $externalPid = $external.Id
    $staleSessionId = "stale-mismatch-$runId"
    $staleMeta = @{
        pid = 3000000000
        child_pid = $externalPid
        child_pid_start_ticks = 1
        provider = "codex"
        session_id = $staleSessionId
        cwd = $cwd
        source = "@cokacmux-new-agent"
        updated_at_epoch_s = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
    } | ConvertTo-Json -Depth 5
    Write-Utf8NoBom (Join-Path $agentsDir "codex-$staleSessionId.json") $staleMeta

    $guardKill = Invoke-Cokacmux $exePath @("killall")
    if ($guardKill.ExitCode -ne 0) {
        Fail "guard killall failed: $($guardKill.Output)"
    }
    if (-not (Get-Process -Id $externalPid -ErrorAction SilentlyContinue)) {
        Fail "mismatched child_pid_start_ticks allowed killall to terminate external process"
    }
    Stop-ProcessIfAlive $externalPid
    $externalPid = $null

    $finalCheck = Invoke-Cokacmux $exePath @("--check")
    if ($finalCheck.ExitCode -ne 0 -or $finalCheck.Output -notmatch "cokacmux --check ok") {
        Fail "--check failed after cleanup: $($finalCheck.Output)"
    }

    [pscustomobject]@{
        Result = "PASS"
        Exe = $exePath
        Root = $root
        DaemonPid = $daemonPid
        ChildPid = $childPid
        TcpPort = $port
        Killall = $kill.Output
        GuardKillall = $guardKill.Output
        Check = $finalCheck.Output
    } | Format-List
}
finally {
    if ($externalPid) {
        Stop-ProcessIfAlive $externalPid
    }
    if ($childPid) {
        Stop-ProcessIfAlive $childPid
    }
    if ($daemonPid) {
        Stop-ProcessIfAlive $daemonPid
    }

    $env:COKACMUX_CONFIG_DIR = $oldConfig
    $env:COKACMUX_HOME = $oldHome
    $env:COKACMUX_DEBUG = $oldDebug
    $env:COKACMUX_FAKE_AGENT_LOG = $oldFakeLog
}
