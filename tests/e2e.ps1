# End-to-end tests for Blitz.exe on a Windows runner.
param([string]$Exe = "target\release\Blitz.exe")
$ErrorActionPreference = "Stop"
$Exe = (Resolve-Path $Exe).Path
$fail = 0
$log = @()
function Check($name, $ok, $detail = "") {
  $line = ("{0} {1} {2}" -f ($(if ($ok) { "PASS" } else { "FAIL" }), $name, $detail))
  Write-Host $line; $script:log += $line
  if (-not $ok) { $script:fail++ }
}
function Alive($procId) { $null -ne (Get-Process -Id $procId -ErrorAction SilentlyContinue) }

$tempRoot = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$outputRoot = if ($env:GITHUB_WORKSPACE) { $env:GITHUB_WORKSPACE } else { (Get-Location).Path }
$work = Join-Path $tempRoot ("blitz-e2e-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory $work | Out-Null
$cfgDir = Join-Path $work "cfg"
$env:BLITZ_CONFIG_DIR = $cfgDir
New-Item -ItemType Directory $cfgDir | Out-Null

$ping = "$env:WINDIR\System32\PING.EXE"
function Dummy($name) {
  $p = Join-Path $work $name
  Copy-Item $ping $p -Force
  return (Start-Process $p -ArgumentList "-t", "127.0.0.1" -WindowStyle Hidden -PassThru)
}
function WriteCfg($checked, $autoBoost = $false, $trim = $false, $whitelist = @()) {
  @{ checked = @($checked | ForEach-Object { $_.ToLower() }); whitelist = @($whitelist); trim_memory = $trim; auto_boost = $autoBoost } |
    ConvertTo-Json | Set-Content -Encoding UTF8 (Join-Path $cfgDir "config.json")
}

# Protected processes snapshot (must all survive)
$protNames = "csrss","wininit","winlogon","services","lsass","smss","svchost","dwm","explorer","fontdrvhost","sihost","ctfmon","MsMpEng"
$prot = Get-Process -Name $protNames -ErrorAction SilentlyContinue
Write-Host "protected processes watched: $($prot.Count)"

# ---- 1. selftest boost: checked dies, unchecked survives, protected-name dummy survives ----
$a = Dummy "dummyA.exe"
$b = Dummy "dummyB.exe"
$x = Dummy "explorer.exe"  # protected by name even though checked
Start-Sleep 1
$aPath = (Join-Path $work "dummyA.exe"); $xPath = (Join-Path $work "explorer.exe")
WriteCfg @($aPath, $xPath) $false $true
$report = Join-Path $work "report.json"
$sw = [Diagnostics.Stopwatch]::StartNew()
$boostProc = Start-Process $Exe -ArgumentList "--selftest-boost", $report -Wait -PassThru
Write-Host "selftest took $($sw.ElapsedMilliseconds) ms"
Start-Sleep 1
$rep = Get-Content $report -Raw | ConvertFrom-Json
Write-Host (Get-Content $report -Raw)
Check "checked dummy killed" (-not (Alive $a.Id))
Check "unchecked dummy survived" (Alive $b.Id)
$unexpectedTrim = @($rep.trimmed_pids | Where-Object { $_ -ne $boostProc.Id -and $_ -ne $a.Id })
Check "memory trim never touches unchecked or protected processes" ($unexpectedTrim.Count -eq 0)
Check "checked-but-protected-name dummy survived" (Alive $x.Id)
Check "report lists killed key" (@($rep.killed) -contains $aPath.ToLower()) ($rep.killed -join ",")
$allowed = @($a.Id)
$bad = @($rep.touched_pids | Where-Object { $allowed -notcontains $_ })
Check "only the checked process was touched" ($bad.Count -eq 0) ("touched=" + ($rep.touched_pids -join ","))
$affectedPids = @($rep.touched_pids) + @($rep.graceful_pids) + @($rep.trimmed_pids)
$affectedProtected = @($prot | Where-Object { $affectedPids -contains $_.Id })
Check "no protected process was targeted or trimmed" ($affectedProtected.Count -eq 0) (($affectedProtected | ForEach-Object { $_.Name }) -join ",")
# Service-host instances may stop naturally during this test. Keep the survival
# assertion for persistent critical processes as well as checking every action.
$persistent = @($prot | Where-Object { $_.Name -in @('csrss','wininit','winlogon','services','lsass') })
$dead = @($persistent | Where-Object { -not (Alive $_.Id) })
Check "persistent critical processes survived" ($dead.Count -eq 0) (($dead | ForEach-Object { $_.Name }) -join ",")
Stop-Process -Id $b.Id, $x.Id -Force -ErrorAction SilentlyContinue

# ---- 1b. unchecked child of a checked parent survives ----
$parentExe = Join-Path $work "dummyP.exe"; Copy-Item "$env:WINDIR\System32\cmd.exe" $parentExe -Force
$childExe = Join-Path $work "dummyQ.exe"; Copy-Item $ping $childExe -Force
$pp = Start-Process $parentExe -ArgumentList "/c", "`"$childExe`" -t 127.0.0.1" -WindowStyle Hidden -PassThru
Start-Sleep 2
$q = Get-CimInstance Win32_Process -Filter "ParentProcessId=$($pp.Id)" | Where-Object { $_.Name -eq "dummyQ.exe" } | Select-Object -First 1
WriteCfg @($parentExe)
Start-Process $Exe -ArgumentList "--selftest-boost", $report -Wait
Check "checked parent killed" (-not (Alive $pp.Id))
Check "unchecked child of checked parent survived" (($null -ne $q) -and (Alive $q.ProcessId)) "child=$($q.ProcessId)"
if ($q) { Stop-Process -Id $q.ProcessId -Force -ErrorAction SilentlyContinue }

# ---- 2. first run: no config -> nothing killed ----
Remove-Item (Join-Path $cfgDir "config.json")
$c = Dummy "dummyC.exe"
Start-Process $Exe -ArgumentList "--selftest-boost", $report -Wait
$rep = Get-Content $report -Raw | ConvertFrom-Json
Check "first run kills nothing" ((@($rep.touched_pids).Count -eq 0) -and (Alive $c.Id))
Stop-Process -Id $c.Id -Force -ErrorAction SilentlyContinue


# ---- 4. Run key ----
$runKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
$oldRun = (Get-ItemProperty $runKey -Name Blitz -ErrorAction SilentlyContinue).Blitz
try {
$p = Start-Process $Exe -ArgumentList "--set-autostart", "on" -Wait -PassThru
$val = (Get-ItemProperty $runKey -Name Blitz -ErrorAction SilentlyContinue).Blitz
Check "autostart on writes Run\Blitz" (($p.ExitCode -eq 0) -and ($val -like "*Blitz.exe*--autostart*")) "$val"
$p = Start-Process $Exe -ArgumentList "--set-autostart", "off" -Wait -PassThru
$val = (Get-ItemProperty $runKey -Name Blitz -ErrorAction SilentlyContinue).Blitz
Check "autostart off removes Run\Blitz" (($p.ExitCode -eq 0) -and ($null -eq $val))

} finally {
  if ($null -ne $oldRun) { Set-ItemProperty $runKey -Name Blitz -Value $oldRun }
  else { Remove-ItemProperty $runKey -Name Blitz -ErrorAction SilentlyContinue }
}

# ---- 5. auto-boost on launch ----
$d = Dummy "dummyD.exe"
$e = Dummy "dummyE.exe"
WriteCfg @((Join-Path $work "dummyD.exe")) $true
Remove-Item (Join-Path $cfgDir "last_boost.json") -ErrorAction SilentlyContinue
$blitz = Start-Process $Exe -ArgumentList "--autostart" -PassThru
$deadline = (Get-Date).AddSeconds(20)
while ((Alive $d.Id) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 300 }
Start-Sleep 2
Check "auto-boost on launch killed checked dummy" (-not (Alive $d.Id))
Check "auto-boost on launch left unchecked dummy" (Alive $e.Id)
Check "auto-boost wrote last_boost.json" (Test-Path (Join-Path $cfgDir "last_boost.json"))
Check "Blitz keeps running in tray" (Alive $blitz.Id)
Stop-Process -Id $blitz.Id, $e.Id -Force -ErrorAction SilentlyContinue

# ---- 6. config persisted under new dir, auto_boost flag kept ----
$saved = Get-Content (Join-Path $cfgDir "config.json") -Raw | ConvertFrom-Json
Check "config.json preserved selection" (@($saved.checked).Count -eq 1 -and $saved.auto_boost)

# ---- 7. selected + whitelisted child is never traversed/terminated ----
$pp = Start-Process $parentExe -ArgumentList "/c", "`"$childExe`" -t 127.0.0.1" -WindowStyle Hidden -PassThru
Start-Sleep 1
$q = Get-CimInstance Win32_Process -Filter "ParentProcessId=$($pp.Id)" | Where-Object Name -eq "dummyQ.exe" | Select-Object -First 1
WriteCfg @($parentExe, $childExe) $false $true @($childExe.ToLower())
Start-Process $Exe -ArgumentList "--selftest-boost", $report -Wait
$rep = Get-Content $report -Raw | ConvertFrom-Json
Check "whitelisted selected child survives checked parent" (($null -ne $q) -and (Alive $q.ProcessId))
Check "whitelisted child not terminated or trimmed" ((@($rep.touched_pids) -notcontains $q.ProcessId) -and (@($rep.trimmed_pids) -notcontains $q.ProcessId))
if ($q) { Stop-Process -Id $q.ProcessId -Force -ErrorAction SilentlyContinue }

# ---- 8. real .lnk launch preserves unicode arguments and working directory ----
$fixture = Join-Path $work "BlitzFixture.exe"
rustc --edition 2021 tests/windows_fixture.rs -o $fixture
if ($LASTEXITCODE -ne 0) { throw "Could not compile native e2e fixture" }
New-Item -ItemType Directory target/e2e -Force | Out-Null
Copy-Item -LiteralPath $fixture -Destination target/e2e/BlitzFixture.exe -Force
$linkDir = Join-Path $work "快捷 方式"
New-Item -ItemType Directory $linkDir | Out-Null
$marker = Join-Path $linkDir "运行 结果.txt"
$shortcut = Join-Path $linkDir "测试 启动.lnk"
. "$PSScriptRoot/shortcut_fixture.ps1"
[BlitzShortcutFixture]::Save($shortcut, $fixture, ('--marker "' + $marker + '" --payload "测试 value"'), $linkDir)
$p = Start-Process $Exe -ArgumentList '--selftest-launch', ('"' + $shortcut + '"') -Wait -PassThru
$deadline = (Get-Date).AddSeconds(8)
while (-not (Test-Path $marker) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 100 }
Check "shortcut starts the target executable" (($p.ExitCode -eq 0) -and (Test-Path $marker))
if (Test-Path $marker) {
  $lines = @(Get-Content $marker)
  Check "shortcut unicode arguments preserved" ($lines[0] -eq '测试 value')
  Check "shortcut working directory preserved" ($lines[1] -eq $linkDir)
}
$p = Start-Process $Exe -ArgumentList '--selftest-launch', ('"' + $fixture + '"') -Wait -PassThru
$deadline = (Get-Date).AddSeconds(8)
while (-not (Test-Path ([IO.Path]::ChangeExtension($fixture, 'started'))) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 100 }
Check "direct exe starts" (($p.ExitCode -eq 0) -and (Test-Path ([IO.Path]::ChangeExtension($fixture, 'started'))))
if (Test-Path ([IO.Path]::ChangeExtension($fixture, 'started'))) {
  Check "direct exe starts in its own directory" ((Get-Content ([IO.Path]::ChangeExtension($fixture, 'started')) -Raw) -eq $work)
}
$p = Start-Process $Exe -ArgumentList '--selftest-launch', ('"' + (Join-Path $work 'missing.exe') + '"') -Wait -PassThru
Check "missing launcher rejected" ($p.ExitCode -ne 0)

# ---- 9. a save/confirmation window preserves the whole executable group ----
WriteCfg @($fixture)
# A visible test window is intentional: it models an unsaved editor refusing WM_CLOSE.
$gui = Start-Process $fixture -ArgumentList '--hold-window' -WindowStyle Normal -PassThru
$worker = Start-Process $fixture -ArgumentList '--idle' -WindowStyle Hidden -PassThru
$deadline = (Get-Date).AddSeconds(8)
do { Start-Sleep -Milliseconds 100; $gui.Refresh() } while ($gui.MainWindowHandle -eq 0 -and (Get-Date) -lt $deadline)
Start-Process $Exe -ArgumentList '--selftest-boost', $report -Wait
$rep = Get-Content $report -Raw | ConvertFrom-Json
Check "window refusing close survives" (Alive $gui.Id)
Check "workers of retained application survive" (Alive $worker.Id)
Check "retained application reported" (@($rep.retained) -contains $fixture.ToLower())
Check "retained application never force-killed" ((@($rep.touched_pids) -notcontains $gui.Id) -and (@($rep.touched_pids) -notcontains $worker.Id))
Stop-Process -Id $gui.Id, $worker.Id -Force -ErrorAction SilentlyContinue

# ---- 10. corrupt current config does not revive old auto-boost ----
$previousAppData = $env:APPDATA
try {
  $env:APPDATA = Join-Path $work 'appdata'
  $env:BLITZ_CONFIG_DIR = $null
  New-Item -ItemType Directory (Join-Path $env:APPDATA 'Blitz'), (Join-Path $env:APPDATA 'OneClickBoost') | Out-Null
  $victim = Dummy 'legacyVictim.exe'
  @{ checked = @((Join-Path $work 'legacyVictim.exe').ToLower()); auto_boost = $true } | ConvertTo-Json |
    Set-Content -Encoding UTF8 (Join-Path $env:APPDATA 'OneClickBoost/config.json')
  Set-Content (Join-Path $env:APPDATA 'Blitz/config.json') 'invalid input'
  $blitz = Start-Process $Exe -ArgumentList '--autostart' -WindowStyle Hidden -PassThru
  Start-Sleep 3
  Check "corrupt current config does not auto-kill old targets" (Alive $victim.Id)
  Check "corrupt configuration backed up" (Test-Path (Join-Path $env:APPDATA 'Blitz/config.invalid.json'))
  Stop-Process -Id $blitz.Id, $victim.Id -Force -ErrorAction SilentlyContinue
} finally { $env:APPDATA = $previousAppData; $env:BLITZ_CONFIG_DIR = $cfgDir }

# ---- 11. replacement instance waits until the original releases its mutex ----
WriteCfg @()
$original = Start-Process $Exe -WindowStyle Hidden -PassThru
Start-Sleep 2
$replacement = Start-Process $Exe -ArgumentList '--wait-for-instance' -WindowStyle Hidden -PassThru
Start-Sleep 1
Stop-Process -Id $original.Id -Force
Start-Sleep 2
Check "replacement instance survives old instance exit" (Alive $replacement.Id)
Stop-Process -Id $replacement.Id -Force -ErrorAction SilentlyContinue

$log | Set-Content (Join-Path $outputRoot "e2e-results.txt")
if ($fail -gt 0) { Write-Host "$fail e2e check(s) failed"; exit 1 }
Write-Host "all e2e checks passed"
