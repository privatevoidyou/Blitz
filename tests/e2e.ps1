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
function WriteCfg($checked, $autoBoost = $false) {
  @{ checked = @($checked | ForEach-Object { $_.ToLower() }); whitelist = @(); trim_memory = $false; auto_boost = $autoBoost } |
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
WriteCfg @($aPath, $xPath)
$report = Join-Path $work "report.json"
$sw = [Diagnostics.Stopwatch]::StartNew()
Start-Process $Exe -ArgumentList "--selftest-boost", $report -Wait
Write-Host "selftest took $($sw.ElapsedMilliseconds) ms"
Start-Sleep 1
$rep = Get-Content $report -Raw | ConvertFrom-Json
Write-Host (Get-Content $report -Raw)
Check "checked dummy killed" (-not (Alive $a.Id))
Check "unchecked dummy survived" (Alive $b.Id)
Check "checked-but-protected-name dummy survived" (Alive $x.Id)
Check "report lists killed key" (@($rep.killed) -contains $aPath.ToLower()) ($rep.killed -join ",")
$allowed = @($a.Id)
$bad = @($rep.touched_pids | Where-Object { $allowed -notcontains $_ })
Check "only the checked process was touched" ($bad.Count -eq 0) ("touched=" + ($rep.touched_pids -join ","))
$dead = @($prot | Where-Object { -not (Alive $_.Id) })
Check "no protected process died" ($dead.Count -eq 0) (($dead | ForEach-Object { $_.Name }) -join ",")
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
$p = Start-Process $Exe -ArgumentList "--set-autostart", "on" -Wait -PassThru
$val = (Get-ItemProperty $runKey -Name Blitz -ErrorAction SilentlyContinue).Blitz
Check "autostart on writes Run\Blitz" (($p.ExitCode -eq 0) -and ($val -like "*Blitz.exe*--autostart*")) "$val"
$p = Start-Process $Exe -ArgumentList "--set-autostart", "off" -Wait -PassThru
$val = (Get-ItemProperty $runKey -Name Blitz -ErrorAction SilentlyContinue).Blitz
Check "autostart off removes Run\Blitz" (($p.ExitCode -eq 0) -and ($null -eq $val))

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

$log | Set-Content (Join-Path $outputRoot "e2e-results.txt")
if ($fail -gt 0) { Write-Host "$fail e2e check(s) failed"; exit 1 }
Write-Host "all e2e checks passed"
