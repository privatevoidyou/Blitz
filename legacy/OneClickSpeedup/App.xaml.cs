using Microsoft.UI.Xaml;
namespace OneClickSpeedup;
public partial class App : Application
{
    private Window? _window;
    [System.Runtime.InteropServices.DllImport("kernel32.dll")] static extern bool FreeConsole();
    public App() { FreeConsole(); InitializeComponent(); }
    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        _window = new MainWindow();
        _window.Activate();
    }
}
