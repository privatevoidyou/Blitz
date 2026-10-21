using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
namespace OneClickSpeedup;
public class ProcGroup
{
    public string Key = "";        // full exe path, or exe name fallback
    public string Display = "";
    public string? Path;
    public bool Foreground;
    public List<int> Pids = new();
}
public static class ProcessScanner
{
    static readonly HashSet<string> Protected = new(StringComparer.OrdinalIgnoreCase)
    {
        "System","Idle","Registry","Memory Compression","Secure System","csrss","wininit","winlogon","services","lsass","lsaiso","smss",
        "svchost","dwm","explorer","fontdrvhost","sihost","ctfmon","MsMpEng","NisSrv","SecurityHealthService","SecurityHealthSystray",
        "audiodg","conhost","dllhost","RuntimeBroker","taskhostw","StartMenuExperienceHost","ShellExperienceHost","SearchHost",
        "SearchApp","TextInputHost","LogonUI","userinit","spoolsv","WmiPrvSE","wudfhost","LockApp","ApplicationFrameHost",
        "SystemSettingsBroker","smartscreen","MpDefenderCoreService","WUDFHost","dasHost","sppsvc"
    };
    public static bool IsProtected(Process p, int selfPid, int mySession)
    {
        if (p.Id == selfPid || p.Id <= 4) return true;
        if (Protected.Contains(p.ProcessName)) return true;
        if (!Native.ProcessIdToSessionId((uint)p.Id, out var s) || s == 0 || s != mySession) return true;
        return false;
    }
    public static List<ProcGroup> Scan(Config cfg)
    {
        int self = Environment.ProcessId;
        Native.ProcessIdToSessionId((uint)self, out var mySession);
        var map = new Dictionary<string, ProcGroup>(StringComparer.OrdinalIgnoreCase);
        foreach (var p in Process.GetProcesses())
        {
            try
            {
                if (IsProtected(p, self, (int)mySession)) continue;
                var path = Native.ImagePath(p.Id);
                if (path == null) continue; // protected/inaccessible process: hide
                if (path.StartsWith(Environment.GetFolderPath(Environment.SpecialFolder.Windows), StringComparison.OrdinalIgnoreCase)
                    && path.Contains(@"\System32\", StringComparison.OrdinalIgnoreCase) && Protected.Contains(Path.GetFileNameWithoutExtension(path))) continue;
                var key = path;
                if (cfg.Whitelist.Contains(key) || cfg.Whitelist.Contains(p.ProcessName)) continue;
                if (!map.TryGetValue(key, out var g))
                {
                    string disp = p.ProcessName;
                    try { var d = FileVersionInfo.GetVersionInfo(path).FileDescription; if (!string.IsNullOrWhiteSpace(d)) disp = d.Trim(); } catch { }
                    g = new ProcGroup { Key = key, Path = path, Display = disp };
                    map[key] = g;
                }
                g.Pids.Add(p.Id);
                if (p.MainWindowHandle != IntPtr.Zero && !string.IsNullOrEmpty(p.MainWindowTitle))
                {
                    g.Foreground = true;
                    if (g.Display == p.ProcessName) g.Display = p.MainWindowTitle;
                }
            }
            catch { }
            finally { p.Dispose(); }
        }
        foreach (var g in map.Values) if (g.Pids.Count > 1) g.Display += $" ({g.Pids.Count})";
        return map.Values.OrderBy(g => g.Display, StringComparer.CurrentCulture).ToList();
    }
}
