param([string]$Exe = "target\release\Blitz.exe")
$Exe = (Resolve-Path $Exe).Path
Add-Type -AssemblyName System.Windows.Forms,System.Drawing
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class W { [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h,int c);
  [DllImport("kernel32.dll")] public static extern IntPtr GetConsoleWindow(); }
"@
[W]::ShowWindow([W]::GetConsoleWindow(), 6) | Out-Null
Get-Process WindowsTerminal,OpenConsole -ErrorAction SilentlyContinue | ForEach-Object { [W]::ShowWindow($_.MainWindowHandle, 6) | Out-Null }
$env:BLITZ_CONFIG_DIR = Join-Path $env:RUNNER_TEMP "blitz-shots"
# a couple of user processes so the list isn't empty; one remembered as checked
$n = Start-Process notepad -PassThru -ErrorAction SilentlyContinue
New-Item -ItemType Directory $env:BLITZ_CONFIG_DIR -Force | Out-Null
function Shot($file, [string[]]$a) {
  $p = Start-Process $Exe -ArgumentList $a -PassThru
  Start-Sleep -Seconds 6
  $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
  $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size)
  $bmp.Save((Join-Path $env:GITHUB_WORKSPACE $file))
  Write-Host "$file alive=$(-not $p.HasExited) ws=$([math]::Round($p.WorkingSet64/1MB,1))MB"
  if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
  Start-Sleep -Seconds 2
}
Shot "screenshot-panel.png" @("--open-panel", "--keep-open")
Shot "screenshot-procs.png" @("--view", "procs", "--keep-open")
Shot "screenshot-settings.png" @("--view", "settings", "--keep-open")
if ($n) { Stop-Process -Id $n.Id -Force -ErrorAction SilentlyContinue }
