using System;
using System.Collections.Generic;
using System.IO;
using System.Text.Json;
namespace OneClickSpeedup;
public class Config
{
    public HashSet<string> Checked { get; set; } = new(StringComparer.OrdinalIgnoreCase);
    public HashSet<string> Whitelist { get; set; } = new(StringComparer.OrdinalIgnoreCase);
    public bool TrimMemory { get; set; }
    public bool AutoBoostOnLaunch { get; set; }
    static string Dir => Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), "OneClickBoost");
    static string FilePath => Path.Combine(Dir, "config.json");
    public static Config Load()
    {
        try
        {
            if (File.Exists(FilePath))
            {
                var c = JsonSerializer.Deserialize<Config>(File.ReadAllText(FilePath)) ?? new Config();
                c.Checked = new(c.Checked, StringComparer.OrdinalIgnoreCase);
                c.Whitelist = new(c.Whitelist, StringComparer.OrdinalIgnoreCase);
                return c;
            }
        }
        catch { }
        return new Config(); // first run: nothing checked
    }
    public void Save()
    {
        try { Directory.CreateDirectory(Dir); File.WriteAllText(FilePath, JsonSerializer.Serialize(this, new JsonSerializerOptions { WriteIndented = true })); }
        catch { }
    }
}
