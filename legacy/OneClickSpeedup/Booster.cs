using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Diagnostics;
using System.Security.Principal;
namespace OneClickSpeedup;
public static class Booster
{
    public static bool IsAdmin()
    {
        using var id = WindowsIdentity.GetCurrent();
        return new WindowsPrincipal(id).IsInRole(WindowsBuiltInRole.Administrator);
    }
    // Only ever touches the PIDs in the checked groups.
    public static (int killed, int denied) Run(List<ProcGroup> targets, bool trim)
    {
        int self = Environment.ProcessId;
        Native.ProcessIdToSessionId((uint)self, out var session);
        var procs = new List<(ProcGroup g, Process p)>();
        foreach (var g in targets)
            foreach (var pid in g.Pids)
            {
                try
                {
                    var p = Process.GetProcessById(pid);
                    // re-verify: still same exe, not protected (PID reuse safety)
                    if (ProcessScanner.IsProtected(p, self, (int)session) ||
                        !string.Equals(Native.ImagePath(pid), g.Path, StringComparison.OrdinalIgnoreCase)) { p.Dispose(); continue; }
                    procs.Add((g, p));
                }
                catch { }
            }
        // polite close first
        bool anyWindow = false;
        foreach (var (_, p) in procs)
            try { if (p.MainWindowHandle != IntPtr.Zero && p.CloseMainWindow()) anyWindow = true; } catch { }
        if (anyWindow) System.Threading.Thread.Sleep(2000);
        var killedGroups = new HashSet<ProcGroup>(); int denied = 0;
        var deniedGroups = new HashSet<ProcGroup>();
        foreach (var (g, p) in procs)
        {
            try
            {
                if (!p.HasExited) { p.Kill(entireProcessTree: true); p.WaitForExit(3000); }
                killedGroups.Add(g);
            }
            catch (Win32Exception) { deniedGroups.Add(g); }
            catch (InvalidOperationException) { killedGroups.Add(g); }
            catch { }
            finally { p.Dispose(); }
        }
        denied = deniedGroups.Count;
        if (trim)
        {
            foreach (var p in Process.GetProcesses())
            {
                try
                {
                    var h = Native.OpenProcess(Native.PROCESS_QUERY_LIMITED_INFORMATION | Native.PROCESS_SET_QUOTA, false, p.Id);
                    if (h != IntPtr.Zero) { Native.EmptyWorkingSet(h); Native.CloseHandle(h); }
                }
                catch { }
                finally { p.Dispose(); }
            }
        }
        killedGroups.ExceptWith(deniedGroups);
        return (killedGroups.Count, denied);
    }
}
