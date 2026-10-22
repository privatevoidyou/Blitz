#![cfg_attr(not(test), windows_subsystem = "windows")]
#[cfg_attr(not(windows), allow(dead_code))]
mod core;
#[cfg(windows)]
mod win;
#[cfg(windows)]
mod ui;

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1).cloned())
}

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    win::free_console();
    let (dir, legacy) = core::config_dirs();

    if let Some(path) = arg_value(&args, "--selftest-launch") {
        let item = core::Launcher { name: "selftest".into(), path };
        std::process::exit(if win::launch_shortcut(&item) { 0 } else { 1 });
    }

    if let Some(report) = arg_value(&args, "--selftest-boost") {
        let cfg = core::Config::load(&dir, legacy.as_deref());
        let rep = win::boost(&cfg, cfg.trim_memory);
        let _ = std::fs::write(report, serde_json::to_string_pretty(&rep).unwrap_or_default());
        return;
    }
    if let Some(v) = arg_value(&args, "--set-autostart") {
        let ok = win::autostart_set(v == "on");
        std::process::exit(if ok && win::autostart_get() == (v == "on") { 0 } else { 1 });
    }
    if !win::single_instance(args.iter().any(|a| a == "--wait-for-instance")) {
        return;
    }
    ui::run(args, dir, legacy);
}

#[cfg(not(windows))]
fn main() {
    let _ = arg_value(&[], "");
    eprintln!("Blitz only runs on Windows.");
}
