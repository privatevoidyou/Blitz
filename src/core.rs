//! Platform-independent logic (unit-tested on any OS).
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub const APP_DIR: &str = "Blitz";
pub const LEGACY_DIR: &str = "OneClickBoost";
pub const RUN_VALUE: &str = "Blitz";

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Checked process keys (lower-cased full exe path).
    #[serde(alias = "Checked")]
    pub checked: BTreeSet<String>,
    /// Never-show keys (lower-cased path or exe name).
    #[serde(alias = "Whitelist")]
    pub whitelist: BTreeSet<String>,
    /// 默认清理系统内存
    #[serde(alias = "TrimMemory")]
    pub trim_memory: bool,
    /// 启动后自动加速
    #[serde(alias = "AutoBoostOnLaunch")]
    pub auto_boost: bool,
}

impl Config {
    fn normalize(mut self) -> Self {
        self.checked = self.checked.iter().map(|s| s.to_lowercase()).collect();
        self.whitelist = self.whitelist.iter().map(|s| s.to_lowercase()).collect();
        self
    }
    /// Load from `<dir>/config.json`, falling back to `<legacy>/config.json` (C# build format).
    /// First run (nothing found / unreadable) yields the default: nothing checked.
    pub fn load(dir: &Path, legacy: Option<&Path>) -> Config {
        let read = |d: &Path| -> Option<Config> {
            let s = std::fs::read_to_string(d.join("config.json")).ok()?;
            serde_json::from_str::<Config>(s.trim_start_matches('\u{feff}')).ok()
        };
        read(dir)
            .or_else(|| legacy.and_then(read))
            .unwrap_or_default()
            .normalize()
    }
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join("config.json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap_or_default())?;
        std::fs::rename(tmp, dir.join("config.json"))
    }
    pub fn set_checked(&mut self, key: &str, on: bool) {
        let k = key.to_lowercase();
        if on {
            self.checked.insert(k);
        } else {
            self.checked.remove(&k);
        }
    }
    pub fn is_checked(&self, key: &str) -> bool {
        self.checked.contains(&key.to_lowercase())
    }
    pub fn is_whitelisted(&self, key: &str, exe_name: &str) -> bool {
        self.whitelist.contains(&key.to_lowercase()) || self.whitelist.contains(&exe_name.to_lowercase())
    }
}

pub fn config_dirs() -> (PathBuf, Option<PathBuf>) {
    if let Ok(d) = std::env::var("BLITZ_CONFIG_DIR") {
        return (PathBuf::from(d), None);
    }
    let base = std::env::var("APPDATA").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."));
    (base.join(APP_DIR), Some(base.join(LEGACY_DIR)))
}

/// Built-in protected list (never shown, never killed). Lower-case, without ".exe".
pub const PROTECTED: &[&str] = &[
    "system", "idle", "registry", "memory compression", "secure system", "csrss", "wininit", "winlogon",
    "services", "lsass", "lsaiso", "smss", "svchost", "dwm", "explorer", "fontdrvhost", "sihost", "ctfmon",
    "msmpeng", "nissrv", "securityhealthservice", "securityhealthsystray", "mpdefendercoreservice", "audiodg",
    "conhost", "dllhost", "runtimebroker", "taskhostw", "startmenuexperiencehost", "shellexperiencehost",
    "searchhost", "searchapp", "searchindexer", "textinputhost", "logonui", "userinit", "spoolsv", "wmiprvse",
    "wudfhost", "lockapp", "applicationframehost", "systemsettingsbroker", "smartscreen", "dashost", "sppsvc",
    "winstore.app", "widgets", "widgetservice", "phoneexperiencehost", "crossdeviceservice", "useroobebroker",
];

pub fn exe_stem(name: &str) -> String {
    let n = name.to_lowercase();
    n.strip_suffix(".exe").map(str::to_string).unwrap_or(n)
}

pub struct ProcFacts<'a> {
    pub pid: u32,
    pub exe_name: &'a str,
    /// Session id, None when it could not be queried.
    pub session: Option<u32>,
    /// Full image path, None when access was denied (protected process).
    pub path: Option<&'a str>,
}

/// True when a process must be hidden and never touched.
pub fn is_protected(p: &ProcFacts, self_pid: u32, my_session: u32) -> bool {
    if p.pid <= 4 || p.pid == self_pid {
        return true;
    }
    if PROTECTED.contains(&exe_stem(p.exe_name).as_str()) {
        return true;
    }
    match p.session {
        Some(s) if s != 0 && s == my_session => {}
        _ => return true,
    }
    p.path.is_none()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub key: String,
    pub path: String,
    pub display: String,
    pub foreground: bool,
    pub pids: Vec<u32>,
}

/// Display rows: header rows have `group == None`.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub label: String,
    pub group: Option<usize>,
    pub checked: bool,
}

/// Build the grouped list (前台应用 / 后台进程), applying search filter and remembered selection.
pub fn build_rows(groups: &[Group], cfg: &Config, query: &str) -> Vec<Row> {
    let q = query.trim().to_lowercase();
    let mut rows = Vec::new();
    for (fg, title) in [(true, "前台应用"), (false, "后台进程")] {
        let items: Vec<usize> = groups
            .iter()
            .enumerate()
            .filter(|(_, g)| g.foreground == fg)
            .filter(|(_, g)| q.is_empty() || g.display.to_lowercase().contains(&q) || g.path.to_lowercase().contains(&q))
            .map(|(i, _)| i)
            .collect();
        if items.is_empty() {
            continue;
        }
        rows.push(Row { label: format!("{title}（{}）", items.len()), group: None, checked: false });
        for i in items {
            let g = &groups[i];
            let label = if g.pids.len() > 1 { format!("{} ({})", g.display, g.pids.len()) } else { g.display.clone() };
            rows.push(Row { label, group: Some(i), checked: cfg.is_checked(&g.key) });
        }
    }
    rows
}

#[cfg_attr(not(test), allow(dead_code))]
/// Groups that will be killed: only the checked ones that are currently running.
pub fn targets<'a>(groups: &'a [Group], cfg: &Config) -> Vec<&'a Group> {
    groups.iter().filter(|g| cfg.is_checked(&g.key)).collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Launcher {
    pub name: String,
    pub path: String,
}

/// Filter launchers by query and return the page plus the page count.
pub fn page_of<'a>(apps: &'a [Launcher], query: &str, page: usize, per_page: usize) -> (Vec<&'a Launcher>, usize) {
    let q = query.trim().to_lowercase();
    let f: Vec<&Launcher> = apps.iter().filter(|a| q.is_empty() || a.name.to_lowercase().contains(&q)).collect();
    let pages = f.len().div_ceil(per_page).max(1);
    let page = page.min(pages - 1);
    (f.into_iter().skip(page * per_page).take(per_page).collect(), pages)
}

/// Skip uninstallers/help/docs shortcuts and dedupe by name (case-insensitive), sorted by name.
pub fn clean_launchers(mut v: Vec<Launcher>) -> Vec<Launcher> {
    const SKIP: &[&str] = &["uninstall", "卸载", "readme", "help", "帮助", "documentation", "release notes"];
    v.retain(|l| {
        let n = l.name.to_lowercase();
        !SKIP.iter().any(|s| n.contains(s))
    });
    v.sort_by_key(|a| a.name.to_lowercase());
    v.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    v
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// Position a panel of size (w,h) next to the tray anchor, inside the work area (taskbar on any edge).
pub fn panel_position(work: Rect, screen: Rect, anchor: Option<Rect>, w: i32, h: i32) -> (i32, i32) {
    let margin = 8;
    let (ax, ay) = match anchor {
        Some(a) => ((a.left + a.right) / 2, (a.top + a.bottom) / 2),
        None => (work.right, work.bottom),
    };
    let mut x = ax - w / 2;
    let mut y = ay - h / 2;
    if work.bottom < screen.bottom {
        y = work.bottom - h - margin; // taskbar at bottom
    } else if work.top > screen.top {
        y = work.top + margin; // top
    } else if work.right < screen.right {
        x = work.right - w - margin; // right
    } else if work.left > screen.left {
        x = work.left + margin; // left
    } else {
        y = work.bottom - h - margin;
    }
    x = x.clamp(work.left + margin, (work.right - w - margin).max(work.left));
    y = y.clamp(work.top + margin, (work.bottom - h - margin).max(work.top));
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("blitz-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn facts<'a>(pid: u32, name: &'a str, session: Option<u32>, path: Option<&'a str>) -> ProcFacts<'a> {
        ProcFacts { pid, exe_name: name, session, path }
    }

    #[test]
    fn protected_system_names_are_hidden() {
        for n in ["csrss.exe", "WININIT.EXE", "winlogon.exe", "smss.exe", "services.exe", "lsass.exe", "svchost.exe",
                  "System", "Idle", "dwm.exe", "explorer.exe", "Registry", "fontdrvhost.exe", "MsMpEng.exe", "sihost.exe", "ctfmon.exe"] {
            assert!(is_protected(&facts(1000, n, Some(1), Some("c:\\x.exe")), 1, 1), "{n}");
        }
    }

    #[test]
    fn self_low_pids_session0_other_session_and_inaccessible_are_protected() {
        assert!(is_protected(&facts(42, "Blitz.exe", Some(1), Some("c:\\b.exe")), 42, 1));
        assert!(is_protected(&facts(4, "foo.exe", Some(1), Some("c:\\f.exe")), 1, 1));
        assert!(is_protected(&facts(500, "foo.exe", Some(0), Some("c:\\f.exe")), 1, 1));
        assert!(is_protected(&facts(500, "foo.exe", Some(2), Some("c:\\f.exe")), 1, 1));
        assert!(is_protected(&facts(500, "foo.exe", None, Some("c:\\f.exe")), 1, 1));
        assert!(is_protected(&facts(500, "foo.exe", Some(1), None), 1, 1));
    }

    #[test]
    fn normal_user_process_is_not_protected() {
        assert!(!is_protected(&facts(500, "chrome.exe", Some(1), Some("c:\\chrome.exe")), 1, 1));
        assert!(!is_protected(&facts(500, "explorerplus.exe", Some(1), Some("c:\\e.exe")), 1, 1));
    }

    #[test]
    fn first_run_has_nothing_checked() {
        let d = tmp("first");
        let c = Config::load(&d, None);
        assert!(c.checked.is_empty() && !c.auto_boost && !c.trim_memory);
    }

    #[test]
    fn save_then_load_roundtrip() {
        let d = tmp("rt");
        let mut c = Config::default();
        c.set_checked("C:\\Apps\\Foo.exe", true);
        c.auto_boost = true;
        c.trim_memory = true;
        c.whitelist.insert("bar.exe".into());
        c.save(&d).unwrap();
        let l = Config::load(&d, None);
        assert_eq!(l, c);
        assert!(l.is_checked("c:\\apps\\FOO.exe"));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn migrates_legacy_csharp_config_when_new_missing() {
        let new = tmp("new");
        let old = tmp("old");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("config.json"),
            "\u{feff}{\"Checked\":[\"C:\\\\A\\\\Foo.exe\"],\"Whitelist\":[\"X.exe\"],\"TrimMemory\":true,\"AutoBoostOnLaunch\":true}").unwrap();
        let c = Config::load(&new, Some(&old));
        assert!(c.is_checked("c:\\a\\foo.exe"));
        assert!(c.is_whitelisted("whatever", "x.exe"));
        assert!(c.trim_memory && c.auto_boost);
        // new config wins once present
        Config::default().save(&new).unwrap();
        assert!(Config::load(&new, Some(&old)).checked.is_empty());
        let _ = std::fs::remove_dir_all(new);
        let _ = std::fs::remove_dir_all(old);
    }

    #[test]
    fn corrupt_config_falls_back_to_defaults() {
        let d = tmp("bad");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("config.json"), "{not json").unwrap();
        assert_eq!(Config::load(&d, None), Config::default());
    }

    fn g(key: &str, fg: bool) -> Group {
        Group { key: key.into(), path: key.into(), display: key.rsplit('\\').next().unwrap().into(), foreground: fg, pids: vec![1] }
    }

    #[test]
    fn selection_persists_and_new_processes_default_unchecked() {
        let d = tmp("sel");
        let mut c = Config::default();
        c.set_checked("c:\\a.exe", true);
        c.save(&d).unwrap();
        let c = Config::load(&d, None);
        let groups = vec![g("c:\\a.exe", true), g("c:\\b.exe", false), g("c:\\new.exe", false)];
        let rows = build_rows(&groups, &c, "");
        let checked: Vec<_> = rows.iter().filter(|r| r.checked).map(|r| r.label.clone()).collect();
        assert_eq!(checked, vec!["a.exe"]);
        let t = targets(&groups, &c);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].key, "c:\\a.exe");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn unchecking_removes_from_targets() {
        let mut c = Config::default();
        c.set_checked("c:\\a.exe", true);
        c.set_checked("C:\\A.EXE", false);
        assert!(targets(&[g("c:\\a.exe", true)], &c).is_empty());
    }

    #[test]
    fn rows_grouped_and_filtered() {
        let groups = vec![g("c:\\fg.exe", true), g("c:\\bg1.exe", false), g("c:\\bg2.exe", false)];
        let rows = build_rows(&groups, &Config::default(), "");
        assert_eq!(rows[0].label, "前台应用（1）");
        assert_eq!(rows[2].label, "后台进程（2）");
        let rows = build_rows(&groups, &Config::default(), "BG2");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].label, "bg2.exe");
    }

    #[test]
    fn launcher_paging_and_cleaning() {
        let apps: Vec<Launcher> = (0..25).map(|i| Launcher { name: format!("App{i:02}"), path: String::new() }).collect();
        let (p, n) = page_of(&apps, "", 0, 12);
        assert_eq!((p.len(), n), (12, 3));
        let (p, _) = page_of(&apps, "", 2, 12);
        assert_eq!(p.len(), 1);
        let (p, _) = page_of(&apps, "", 99, 12);
        assert_eq!(p[0].name, "App24");
        let (p, n) = page_of(&apps, "app1", 0, 12);
        assert_eq!((p.len(), n), (10, 1));
        let (p, n) = page_of(&[], "", 0, 12);
        assert_eq!((p.len(), n), (0, 1));
        let v = clean_launchers(vec![
            Launcher { name: "Zed".into(), path: "1".into() },
            Launcher { name: "卸载 Foo".into(), path: "2".into() },
            Launcher { name: "zed".into(), path: "3".into() },
            Launcher { name: "Alpha".into(), path: "4".into() },
        ]);
        assert_eq!(v.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), vec!["Alpha", "Zed"]);
    }

    #[test]
    fn panel_sits_above_bottom_taskbar_near_tray() {
        let screen = Rect { left: 0, top: 0, right: 1920, bottom: 1080 };
        let work = Rect { left: 0, top: 0, right: 1920, bottom: 1032 };
        let tray = Rect { left: 1700, top: 1040, right: 1724, bottom: 1064 };
        let (x, y) = panel_position(work, screen, Some(tray), 360, 480);
        assert_eq!(y, 1032 - 480 - 8);
        assert!(x + 360 <= 1920 - 8 && x >= 0);
        // top taskbar
        let work = Rect { left: 0, top: 48, right: 1920, bottom: 1080 };
        assert_eq!(panel_position(work, screen, None, 360, 480).1, 56);
    }
}
