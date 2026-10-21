using System;
using Microsoft.Win32;
namespace OneClickSpeedup;
public static class AutoStart
{
    const string KeyPath = @"Software\Microsoft\Windows\CurrentVersion\Run";
    const string Name = "OneClickSpeedup";
    public static bool IsEnabled()
    {
        try { using var k = Registry.CurrentUser.OpenSubKey(KeyPath); return k?.GetValue(Name) != null; } catch { return false; }
    }
    public static bool Set(bool on)
    {
        try
        {
            using var k = Registry.CurrentUser.CreateSubKey(KeyPath);
            if (on) k.SetValue(Name, $"\"{Environment.ProcessPath}\" --autostart");
            else k.DeleteValue(Name, false);
            return true;
        }
        catch { return false; }
    }
}
