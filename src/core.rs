//! Platform-independent logic (unit-tested on any OS).
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

pub const APP_DIR: &str = "Blitz";
pub const LEGACY_DIR: &str = "OneClickBoost";
pub const RUN_VALUE: &str = "Blitz";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Only shortcuts explicitly added by the user appear on the home page.
    pub shortcuts: Vec<Launcher>,
    /// 结束所选程序时连同其子进程（经创建时间校验，跳过受保护/白名单进程）
    pub kill_tree: bool,
    /// 管理员模式下额外执行系统级整理（全部工作集、已修改页、低优先级备用页）
    pub deep_trim: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            checked: BTreeSet::new(),
            whitelist: BTreeSet::new(),
            trim_memory: false,
            auto_boost: false,
            shortcuts: Vec::new(),
            kill_tree: true,
            deep_trim: false,
        }
    }
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
        if dir.join("config.json").exists() {
            if let Some(cfg) = read(dir) {
                return cfg.normalize();
            }
            // A broken current config must never revive old auto-boost targets.
            let backup = dir.join("config.invalid.json");
            if !backup.exists() {
                let _ = std::fs::copy(dir.join("config.json"), backup);
            }
            return Config::default();
        }
        legacy.and_then(read).unwrap_or_default().normalize()
    }
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join("config.json.tmp");
        use std::io::Write;
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(serde_json::to_string_pretty(self).map_err(std::io::Error::other)?.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(tmp, dir.join("config.json"))
    }
    pub fn add_shortcut(&mut self, item: Launcher) -> Result<bool, &'static str> {
        if !valid_launcher_file(&item.path) {
            return Err("请选择存在的 .lnk 快捷方式或 .exe 程序");
        }
        if self.shortcuts.iter().any(|x| x.path.eq_ignore_ascii_case(&item.path)) {
            return Ok(false);
        }
        self.shortcuts.push(item);
        Ok(true)
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
    "backgroundtaskhost", "shellhost", "chxsmartscreen", "werfault", "werfaultsecure",
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
    /// Foreground applications that are verified ancestors of this background group.
    pub related: Vec<String>,
    /// Other groups that are verified descendants (ended together when `kill_tree` is on).
    pub children: Vec<String>,
    /// Sum of private (commit) bytes of all processes in the group.
    pub mem: u64,
}

/// Display rows: header rows have `group == None`.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub label: String,
    pub group: Option<usize>,
    pub checked: bool,
    pub hint: String,
    pub size: String,
}

/// Human-readable memory size: "" for 0, "512 MB", "1.3 GB".
pub fn fmt_size(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    if bytes == 0 {
        String::new()
    } else if bytes >= 1024 * MB {
        format!("{:.1} GB", bytes as f64 / (1024 * MB) as f64)
    } else {
        format!("{} MB", bytes.div_ceil(MB))
    }
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
        rows.push(Row { label: format!("{title}（{}）", items.len()), group: None, checked: false, hint: String::new(), size: String::new() });
        for i in items {
            let g = &groups[i];
            let label = if g.pids.len() > 1 { format!("{} ({})", g.display, g.pids.len()) } else { g.display.clone() };
            let mut hints = Vec::new();
            if !g.children.is_empty() {
                hints.push(format!("{}：{}", if cfg.kill_tree { "连同结束" } else { "子进程" }, g.children.join("、")));
            }
            if !g.related.is_empty() {
                hints.push(format!("进程树关联：{}", g.related.join("、")));
            }
            rows.push(Row { label, group: Some(i), checked: cfg.is_checked(&g.key), hint: hints.join(" · "), size: fmt_size(g.mem) });
        }
    }
    rows
}

#[cfg_attr(not(test), allow(dead_code))]
/// Groups that will be killed: only the checked ones that are currently running.
pub fn targets<'a>(groups: &'a [Group], cfg: &Config) -> Vec<&'a Group> {
    groups.iter().filter(|g| cfg.is_checked(&g.key)).collect()
}

/// One process in a snapshot. `created` is the creation FILETIME; 0 means unknown.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Node {
    pub pid: u32,
    pub parent: u32,
    pub created: u64,
}

/// Parent/child index over one process snapshot.
pub struct Tree<'a> {
    nodes: HashMap<u32, &'a Node>,
    children: HashMap<u32, Vec<&'a Node>>,
}

impl<'a> Tree<'a> {
    pub fn new(nodes: &'a [Node]) -> Self {
        let mut children: HashMap<u32, Vec<&Node>> = HashMap::new();
        for n in nodes.iter().filter(|n| n.pid != n.parent) {
            children.entry(n.parent).or_default().push(n);
        }
        Tree { nodes: nodes.iter().map(|n| (n.pid, n)).collect(), children }
    }

    /// Verified descendants of `roots` as `(pid, root pid)`, parents always before children.
    ///
    /// Windows reuses process ids and never updates a child's recorded parent id, so a
    /// child only counts when both creation times are known and the child was not created
    /// before its parent. `follow(pid)` must allow a process for it (and anything below it)
    /// to be included.
    pub fn descendants(&self, roots: &[u32], follow: impl Fn(u32) -> bool) -> Vec<(u32, u32)> {
        let mut seen: HashSet<u32> = roots.iter().copied().collect();
        let mut stack: Vec<(u32, u32)> = roots.iter().rev().map(|&r| (r, r)).collect();
        let mut out = Vec::new();
        while let Some((pid, root)) = stack.pop() {
            let Some(parent) = self.nodes.get(&pid) else { continue };
            if parent.created == 0 {
                continue;
            }
            for child in self.children.get(&pid).into_iter().flatten() {
                if child.created == 0 || child.created < parent.created || !follow(child.pid) || !seen.insert(child.pid) {
                    continue;
                }
                out.push((child.pid, root));
                stack.push((child.pid, root));
            }
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Launcher {
    pub name: String,
    pub path: String,
}

pub fn valid_launcher_file(path: &str) -> bool {
    let p = Path::new(path);
    p.is_absolute() && p.is_file() && p.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("exe") || ext.eq_ignore_ascii_case("lnk"))
}

pub fn in_directory(path: &str, dir: &str) -> bool {
    let path = path.replace('/', "\\").to_lowercase();
    let prefix = format!("{}\\", dir.replace('/', "\\").trim_end_matches('\\').to_lowercase());
    path.starts_with(&prefix)
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
        assert!(c.kill_tree && !c.deep_trim);
    }

    #[test]
    fn older_configs_gain_process_tree_default() {
        let d = tmp("old-fields");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("config.json"), "{\"checked\":[\"c:\\\\a.exe\"],\"trim_memory\":true}").unwrap();
        let c = Config::load(&d, None);
        assert!(c.kill_tree && !c.deep_trim && c.trim_memory && c.is_checked("c:\\a.exe"));
        let _ = std::fs::remove_dir_all(d);
    }

    fn n(pid: u32, parent: u32, created: u64) -> Node {
        Node { pid, parent, created }
    }

    #[test]
    fn tree_walks_verified_descendants_in_parent_order() {
        let nodes = [n(10, 1, 100), n(11, 10, 110), n(12, 11, 120), n(13, 10, 130), n(20, 1, 50)];
        let tree = Tree::new(&nodes);
        let d = tree.descendants(&[10], |_| true);
        let pids: Vec<u32> = d.iter().map(|x| x.0).collect();
        assert_eq!(pids.len(), 3);
        assert!(d.iter().all(|x| x.1 == 10));
        let pos = |p: u32| pids.iter().position(|&x| x == p).unwrap();
        assert!(pos(11) < pos(12), "parent must come before its child");
        assert!(tree.descendants(&[20], |_| true).is_empty());
    }

    #[test]
    fn tree_rejects_reused_parent_ids_and_unknown_times() {
        // 31 claims parent 30 but was created before 30 existed: 30 reused an old pid.
        let nodes = [n(30, 1, 500), n(31, 30, 400), n(32, 30, 0), n(33, 30, 600)];
        let tree = Tree::new(&nodes);
        assert_eq!(tree.descendants(&[30], |_| true), vec![(33, 30)]);
        let unknown_root = [n(40, 1, 0), n(41, 40, 10)];
        assert!(Tree::new(&unknown_root).descendants(&[40], |_| true).is_empty());
    }

    #[test]
    fn tree_stops_at_processes_it_may_not_follow() {
        let nodes = [n(50, 1, 1), n(51, 50, 2), n(52, 51, 3), n(53, 50, 4)];
        let tree = Tree::new(&nodes);
        let d: Vec<u32> = tree.descendants(&[50], |pid| pid != 51).iter().map(|x| x.0).collect();
        assert_eq!(d, vec![53], "a protected child also shields its own children");
        // A root reachable from another root is not reported twice.
        let both: Vec<u32> = tree.descendants(&[50, 51], |_| true).iter().map(|x| x.0).collect();
        assert_eq!(both.iter().filter(|&&p| p == 51).count(), 0);
        assert_eq!(both.iter().filter(|&&p| p == 52).count(), 1);
    }

    #[test]
    fn sizes_and_tree_hints_render() {
        assert_eq!(fmt_size(0), "");
        assert_eq!(fmt_size(5 * 1024 * 1024), "5 MB");
        assert_eq!(fmt_size(3 * 1024 * 1024 * 1024 / 2), "1.5 GB");
        let mut parent = g("c:\\app\\main.exe", true);
        parent.children = vec!["crashpad_handler".into()];
        parent.mem = 200 * 1024 * 1024;
        let mut cfg = Config::default();
        let rows = build_rows(&[parent.clone()], &cfg, "");
        assert_eq!(rows[1].hint, "连同结束：crashpad_handler");
        assert_eq!(rows[1].size, "200 MB");
        cfg.kill_tree = false;
        assert_eq!(build_rows(&[parent], &cfg, "")[1].hint, "子进程：crashpad_handler");
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

    #[test]
    fn corrupt_current_config_never_revives_legacy_auto_boost() {
        let current = tmp("broken-current");
        let old = tmp("old-autoboost");
        let mut legacy = Config::default();
        legacy.set_checked("c:\\apps\\editor.exe", true);
        legacy.auto_boost = true;
        legacy.save(&old).unwrap();
        std::fs::create_dir_all(&current).unwrap();
        std::fs::write(current.join("config.json"), "broken input").unwrap();
        let loaded = Config::load(&current, Some(&old));
        assert!(!loaded.auto_boost && loaded.checked.is_empty());
        assert_eq!(std::fs::read_to_string(current.join("config.invalid.json")).unwrap(), "broken input");
    }

    #[test]
    fn shortcuts_are_opt_in_deduplicated_and_persisted() {
        let dir = tmp("shortcuts");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("示例 快捷方式.lnk");
        std::fs::write(&path, []).unwrap();
        let item = Launcher { name: "示例".into(), path: path.to_string_lossy().into() };
        let mut cfg = Config::default();
        assert!(cfg.shortcuts.is_empty());
        assert_eq!(cfg.add_shortcut(item.clone()), Ok(true));
        assert_eq!(cfg.add_shortcut(item), Ok(false));
        cfg.save(&dir).unwrap();
        assert_eq!(Config::load(&dir, None).shortcuts, cfg.shortcuts);
        assert!(cfg.add_shortcut(Launcher { name: "bad".into(), path: "https://example.com/launch.exe".into() }).is_err());
        let unsafe_file = dir.join("document.txt");
        std::fs::write(&unsafe_file, []).unwrap();
        assert!(!valid_launcher_file(&unsafe_file.to_string_lossy()));
        assert!(in_directory("C:\\Windows\\System32\\ShellHost.exe", "c:\\windows"));
        assert!(!in_directory("C:\\WindowsApps\\app.exe", "C:\\Windows"));
        cfg.shortcuts.clear();
        cfg.save(&dir).unwrap();
        assert!(Config::load(&dir, None).shortcuts.is_empty());
    }

    #[test]
    fn failed_save_never_overwrites_the_last_complete_config() {
        let dir = tmp("failed-save");
        let mut cfg = Config::default();
        cfg.set_checked("c:\\apps\\editor.exe", true);
        cfg.save(&dir).unwrap();
        std::fs::create_dir(dir.join("config.json.tmp")).unwrap();
        let previous = cfg.clone();
        cfg.set_checked("c:\\apps\\editor.exe", false);
        assert!(cfg.save(&dir).is_err());
        assert_eq!(Config::load(&dir, None), previous);
    }

    fn g(key: &str, fg: bool) -> Group {
        Group { key: key.into(), path: key.into(), display: key.rsplit('\\').next().unwrap().into(), foreground: fg, pids: vec![1], related: vec![], children: vec![], mem: 0 }
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
    fn checked_and_unchecked_states_survive_repeated_save_and_filtered_refresh() {
        let d = tmp("toggle-refresh");
        let groups = vec![g("c:\\apps\\Code.exe", true), g("c:\\apps\\Grok Bot.exe", true)];
        let mut cfg = Config::default();
        for on in [true, false, true, false] {
            cfg.set_checked(&groups[0].key, on);
            cfg.save(&d).unwrap();
            cfg = Config::load(&d, None);
            let filtered = build_rows(&groups, &cfg, "code");
            assert_eq!(filtered.len(), 2);
            assert_eq!(filtered[1].checked, on);
            let all = build_rows(&groups, &cfg, "");
            assert_eq!(all[1].checked, on);
            assert!(!all[2].checked, "unrelated process must remain unchecked");
        }
        let _ = std::fs::remove_dir_all(d);
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
