# Exercises the supervisor's crash recovery end to end:
#   3 crashes -> safe mode, 3 more -> give up (Explorer restored), then `start` recovers.
# Usage: scripts\test-recovery.ps1 [-Profile debug|release] [-HideTaskbar]
param(
    [string]$Profile = "debug",
    [switch]$HideTaskbar
)
$ErrorActionPreference = "Stop"
$bin = Join-Path $PSScriptRoot "..\target\$Profile"
$ctl = Join-Path $bin "fsh-ctl.exe"

function Status { (& $ctl shell status 2>$null | Out-String) }
function SessionStatus { & $ctl status | ConvertFrom-Json }

function Wait-ShellUp([int]$notPid, [int]$timeoutMs = 10000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $timeoutMs) {
        $s = SessionStatus
        if ($s.shell.pid -and $s.shell.pid -ne $notPid) { return $s }
        Start-Sleep -Milliseconds 100
    }
    throw "shell did not come back within $timeoutMs ms"
}

function Crash-And-Time {
    $before = (SessionStatus).shell.pid
    $sw = [Diagnostics.Stopwatch]::StartNew()
    & $ctl debug crash | Out-Null
    $s = Wait-ShellUp $before
    Write-Host "  crashed pid $before -> new pid $($s.shell.pid) in $($sw.ElapsedMilliseconds) ms (safe_mode=$($s.session.safe_mode))"
    return $s
}

$args = @()
if (-not $HideTaskbar) { $args += "--keep-explorer-taskbar" }
$session = Start-Process (Join-Path $bin "fsh-session.exe") -ArgumentList $args -WindowStyle Hidden -PassThru
try {
    $s = Wait-ShellUp 0
    Write-Host "shell up (pid $($s.shell.pid))"

    Write-Host "normal mode crashes:"
    1..2 | ForEach-Object { Crash-And-Time | Out-Null }
    $s = Crash-And-Time
    if (-not $s.session.safe_mode) { throw "expected safe mode after 3 crashes" }
    Write-Host "OK: entered safe mode"

    Write-Host "safe mode crashes:"
    1..2 | ForEach-Object { Crash-And-Time | Out-Null }
    $before = (SessionStatus).shell.pid
    & $ctl debug crash | Out-Null
    $sw = [Diagnostics.Stopwatch]::StartNew()
    do { Start-Sleep -Milliseconds 100; $s = SessionStatus } until ($s.session.state -eq "gave-up" -or $sw.ElapsedMilliseconds -gt 10000)
    if ($s.session.state -ne "gave-up") { throw "expected gave-up, got $($s.session.state)" }
    Write-Host "OK: gave up after safe mode crash loop"

    & $ctl start | Out-Null
    $s = Wait-ShellUp $before
    if ($s.session.safe_mode) { throw "start should reset to normal mode" }
    Write-Host "OK: start recovered in normal mode (pid $($s.shell.pid))"
    Write-Host "ALL RECOVERY CHECKS PASSED"
}
finally {
    & $ctl quit | Out-Null
    if (-not $session.WaitForExit(8000)) { $session.Kill() }
}
