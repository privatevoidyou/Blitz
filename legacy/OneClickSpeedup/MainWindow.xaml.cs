using System;
using System.Collections.Generic;
using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Threading.Tasks;
using Microsoft.UI;
using Microsoft.UI.Composition.SystemBackdrops;
using Microsoft.UI.Text;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Imaging;
using Windows.UI.Text;
using System.Runtime.InteropServices.WindowsRuntime;

namespace OneClickSpeedup;

public class ProcItem : INotifyPropertyChanged
{
    public event PropertyChangedEventHandler? PropertyChanged;
    public ProcGroup? Group { get; init; }
    public string Label { get; init; } = "";
    public bool IsHeader => Group == null;
    public Visibility CheckVisibility => IsHeader ? Visibility.Collapsed : Visibility.Visible;
    public FontWeight Weight => IsHeader ? FontWeights.SemiBold : FontWeights.Normal;
    public double TextOpacity => IsHeader ? 0.7 : 1.0;
    public ImageSource? Icon { get; set; }
    public Action<ProcItem>? OnChanged;
    bool _checked;
    public bool? IsChecked
    {
        get => _checked;
        set { var v = value == true; if (v == _checked) return; _checked = v; PropertyChanged?.Invoke(this, new(nameof(IsChecked))); OnChanged?.Invoke(this); }
    }
    public bool Checked => _checked;
}

public sealed partial class MainWindow : Window
{
    readonly Config _cfg = Config.Load();
    List<ProcItem> _all = new();
    readonly ObservableCollection<ProcItem> _view = new();
    readonly DispatcherTimer _timer = new() { Interval = TimeSpan.FromSeconds(2) };
    bool _loading;

    public MainWindow()
    {
        InitializeComponent();
        Title = "一键电脑加速";
        ExtendsContentIntoTitleBar = true;
        SetTitleBar(AppTitleBar);
        AppWindow.TitleBar.PreferredHeightOption = TitleBarHeightOption.Standard;
        AppWindow.TitleBar.ButtonBackgroundColor = Colors.Transparent;
        AppWindow.TitleBar.ButtonInactiveBackgroundColor = Colors.Transparent;
        AppWindow.TitleBar.ButtonHoverBackgroundColor = Windows.UI.Color.FromArgb(40, 255, 255, 255);
        AppWindow.TitleBar.ButtonInactiveForegroundColor = Colors.Gray;
        if (MicaController.IsSupported()) SystemBackdrop = new MicaBackdrop();
        else Outer.Background = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 32, 32, 32));
        var scale = GetDpiForWindowSafe();
        AppWindow.Resize(new Windows.Graphics.SizeInt32((int)(320 * scale), (int)(460 * scale)));
        if (AppWindow.Presenter is OverlappedPresenter op) op.IsMaximizable = false;
        AppWindow.TitleBar.ButtonForegroundColor = Colors.White;
        List.ItemsSource = _view;
        _loading = true;
        TrimMemory.IsChecked = _cfg.TrimMemory;
        AutoStartSw.IsOn = AutoStart.IsEnabled();
        AutoBoostSw.IsOn = _cfg.AutoBoostOnLaunch;
        TrimDefaultSw.IsOn = _cfg.TrimMemory;
        _loading = false;
        Reload();
        UpdateMem();
        _timer.Tick += (_, _) => UpdateMem();
        _timer.Start();
        var args = Environment.GetCommandLineArgs();
        if (args.Contains("--open-settings"))
            Root.Loaded += (_, _) => SettingsFlyout.ShowAt(SettingsBtn);
        else if (_cfg.AutoBoostOnLaunch && _all.Any(i => i.Checked))
            Root.Loaded += (_, _) => Boost_Click(this, new RoutedEventArgs());
    }

    void Settings_Toggled(object sender, RoutedEventArgs e)
    {
        if (_loading) return;
        if (sender == AutoStartSw) { if (!AutoStart.Set(AutoStartSw.IsOn)) { _loading = true; AutoStartSw.IsOn = AutoStart.IsEnabled(); _loading = false; } }
        else if (sender == AutoBoostSw) { _cfg.AutoBoostOnLaunch = AutoBoostSw.IsOn; _cfg.Save(); }
        else if (sender == TrimDefaultSw) { _cfg.TrimMemory = TrimDefaultSw.IsOn; _cfg.Save(); _loading = true; TrimMemory.IsChecked = TrimDefaultSw.IsOn; _loading = false; }
    }

    [System.Runtime.InteropServices.DllImport("user32.dll")] static extern uint GetDpiForWindow(IntPtr hwnd);
    double GetDpiForWindowSafe()
    {
        try { var h = WinRT.Interop.WindowNative.GetWindowHandle(this); var d = GetDpiForWindow(h); return d == 0 ? 1 : d / 96.0; } catch { return 1; }
    }

    void UpdateMem()
    {
        var (load, _) = Native.Mem();
        MemBar.Value = load;
        MemText.Text = $"内存使用率 {load}%";
    }

    void Reload()
    {
        var groups = ProcessScanner.Scan(_cfg);
        var items = new List<ProcItem>();
        void Add(string header, IEnumerable<ProcGroup> gs)
        {
            var list = gs.ToList();
            if (list.Count == 0) return;
            items.Add(new ProcItem { Label = $"{header}（{list.Count}）" });
            foreach (var g in list)
            {
                var it = new ProcItem { Group = g, Label = g.Display };
                // restore only previously-checked processes; new ones default unchecked
                if (_cfg.Checked.Contains(g.Key)) it.IsChecked = true;
                it.OnChanged = OnItemChanged;
                items.Add(it);
                _ = LoadIconAsync(it);
            }
        }
        Add("前台应用", groups.Where(g => g.Foreground));
        Add("后台进程", groups.Where(g => !g.Foreground));
        _all = items;
        ApplyFilter();
    }

    void OnItemChanged(ProcItem it)
    {
        if (it.Group == null) return;
        if (it.Checked) _cfg.Checked.Add(it.Group.Key); else _cfg.Checked.Remove(it.Group.Key);
        _cfg.Save();
    }

    async Task LoadIconAsync(ProcItem it)
    {
        try
        {
            if (it.Group?.Path == null) return;
            byte[] png = await Task.Run(() =>
            {
                using var ico = System.Drawing.Icon.ExtractAssociatedIcon(it.Group.Path);
                if (ico == null) return Array.Empty<byte>();
                using var bmp = ico.ToBitmap(); using var ms = new MemoryStream();
                bmp.Save(ms, System.Drawing.Imaging.ImageFormat.Png); return ms.ToArray();
            });
            if (png.Length == 0) return;
            var img = new BitmapImage();
            using var ras = new Windows.Storage.Streams.InMemoryRandomAccessStream();
            await ras.WriteAsync(png.AsBuffer());
            ras.Seek(0);
            await img.SetSourceAsync(ras);
            it.Icon = img;
            var idx = _view.IndexOf(it);
            if (idx >= 0) { _view[idx] = it; }
        }
        catch { }
    }

    void ApplyFilter()
    {
        var q = Search.Text?.Trim() ?? "";
        _view.Clear();
        foreach (var it in _all)
            if (it.IsHeader || q.Length == 0 || it.Label.Contains(q, StringComparison.CurrentCultureIgnoreCase)
                || (it.Group?.Path?.Contains(q, StringComparison.OrdinalIgnoreCase) ?? false))
                _view.Add(it);
    }

    void Search_TextChanged(AutoSuggestBox sender, AutoSuggestBoxTextChangedEventArgs args) => ApplyFilter();

    void TrimMemory_Changed(object sender, RoutedEventArgs e)
    {
        if (_loading) return;
        _cfg.TrimMemory = TrimMemory.IsChecked == true; _cfg.Save();
        _loading = true; TrimDefaultSw.IsOn = _cfg.TrimMemory; _loading = false;
    }

    void Item_RightTapped(object sender, Microsoft.UI.Xaml.Input.RightTappedRoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.Tag is not ProcItem it || it.Group == null) return;
        var flyout = new MenuFlyout();
        var mi = new MenuFlyoutItem { Text = "加入白名单（永不显示）" };
        mi.Click += (_, _) =>
        {
            _cfg.Whitelist.Add(it.Group.Key); _cfg.Checked.Remove(it.Group.Key); _cfg.Save(); Reload();
        };
        flyout.Items.Add(mi);
        flyout.ShowAt((FrameworkElement)sender, e.GetPosition((FrameworkElement)sender));
    }

    async void Boost_Click(object sender, RoutedEventArgs e)
    {
        BoostBtn.IsEnabled = false; Status.Text = "正在加速…"; AdminBtn.Visibility = Visibility.Collapsed;
        var targets = _all.Where(i => i.Group != null && i.Checked).Select(i => i.Group!).ToList();
        var before = Native.Mem().availMb;
        bool trim = TrimMemory.IsChecked == true;
        var (killed, denied) = await Task.Run(() => Booster.Run(targets, trim));
        await Task.Delay(500);
        var after = Native.Mem().availMb;
        var freed = after > before ? after - before : 0;
        Status.Text = $"已结束 {killed} 个程序，释放内存约 {freed} MB" + (denied > 0 ? $"；{denied} 个需管理员权限" : "");
        if (denied > 0 && !Booster.IsAdmin()) AdminBtn.Visibility = Visibility.Visible;
        Reload(); UpdateMem();
        BoostBtn.IsEnabled = true;
    }

    void AdminBtn_Click(object sender, RoutedEventArgs e)
    {
        try
        {
            Process.Start(new ProcessStartInfo(Environment.ProcessPath!) { UseShellExecute = true, Verb = "runas" });
            Application.Current.Exit();
        }
        catch { Status.Text = "已取消管理员重启"; }
    }
}
