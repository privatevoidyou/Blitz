//! Win32 layer: processes, kill, memory, icons, shortcuts, autostart, tray.
#![cfg(windows)]
use crate::core::{self, Config, Group, Launcher, ProcFacts};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use windows::core::{w, Interface, PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Diagnostics::ToolHelp::*;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, STGM_READ};
use windows::Win32::Security::{AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY};
use windows::Win32::System::ProcessStatus::{EmptyWorkingSet, GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Registry::*;
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::Controls::Dialogs::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub struct Snap {
    pub pid: u32,
    pub parent: u32,
    pub name: String,
}

pub fn snapshot() -> Vec<Snap> {
    let mut out = Vec::new();
    unsafe {
        let Ok(h) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        if Process32FirstW(h, &mut e).is_ok() {
            loop {
                let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                out.push(Snap { pid: e.th32ProcessID, parent: e.th32ParentProcessID, name: String::from_utf16_lossy(&e.szExeFile[..len]) });
                if Process32NextW(h, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(h);
    }
    out
}

pub fn session_of(pid: u32) -> Option<u32> {
    let mut s = 0u32;
    unsafe { ProcessIdToSessionId(pid, &mut s).ok().map(|_| s) }
}

struct ProcessHandle(HANDLE);
impl Drop for ProcessHandle {
    fn drop(&mut self) { unsafe { let _ = CloseHandle(self.0); } }
}

fn open(pid: u32, rights: PROCESS_ACCESS_RIGHTS) -> Option<ProcessHandle> {
    unsafe { OpenProcess(rights, false, pid).ok().map(ProcessHandle) }
}

fn handle_path(h: HANDLE) -> Option<String> {
    let mut buf = [0u16; 32768];
    let mut n = buf.len() as u32;
    unsafe { QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut n).ok()?; }
    Some(String::from_utf16_lossy(&buf[..n as usize]))
}

fn critical(h: HANDLE) -> bool {
    let mut critical = BOOL(1);
    // Fail closed if Windows cannot confirm that this process is non-critical.
    unsafe { IsProcessCritical(h, &mut critical).is_err() || critical.as_bool() }
}

/// Creation time as a FILETIME tick count; 0 when unknown.
fn created_at(h: HANDLE) -> u64 {
    let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe {
        if GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u).is_err() { return 0; }
    }
    ((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64
}

/// (private commit bytes, working-set bytes)
fn memory(h: HANDLE) -> (u64, u64) {
    let mut m = PROCESS_MEMORY_COUNTERS_EX { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32, ..Default::default() };
    unsafe {
        if GetProcessMemoryInfo(h, &mut m as *mut _ as *mut PROCESS_MEMORY_COUNTERS, m.cb).is_err() { return (0, 0); }
    }
    (m.PrivateUsage as u64, m.WorkingSetSize as u64)
}

fn windows_path(path: &str) -> bool {
    std::env::var("WINDIR").map(|dir| core::in_directory(path, &dir)).unwrap_or(true)
}

/// Everything Blitz needs to know about one process, read through a single handle.
pub struct Probe {
    pub pid: u32,
    pub parent: u32,
    pub name: String,
    pub path: Option<String>,
    pub created: u64,
    pub critical: bool,
    pub private: u64,
    pub session: Option<u32>,
}

impl Probe {
    fn key(&self) -> String { self.path.as_deref().unwrap_or_default().to_lowercase() }
    fn node(&self) -> core::Node { core::Node { pid: self.pid, parent: self.parent, created: self.created } }
}

pub fn probe_all() -> Vec<Probe> {
    snapshot().into_iter().map(|s| {
        let h = if s.pid > 4 { open(s.pid, PROCESS_QUERY_LIMITED_INFORMATION) } else { None };
        let (path, created, critical, private) = match &h {
            Some(h) => (handle_path(h.0), created_at(h.0), critical(h.0), memory(h.0).0),
            None => (None, 0, true, 0),
        };
        Probe { pid: s.pid, parent: s.parent, session: session_of(s.pid), name: s.name, path, created, critical, private }
    }).collect()
}

fn protected(p: &Probe, self_pid: u32, my_session: u32) -> bool {
    core::is_protected(&ProcFacts { pid: p.pid, exe_name: &p.name, session: p.session, path: p.path.as_deref() }, self_pid, my_session)
        || p.path.as_deref().is_some_and(windows_path)
        || p.critical
}

/// True when Windows refuses to open the process at all (needs administrator rights),
/// as opposed to the process having exited or been replaced in the meantime.
fn access_denied(pid: u32) -> bool {
    unsafe {
        match OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            Ok(h) => { let _ = CloseHandle(h); false }
            Err(e) => e.code() == E_ACCESSDENIED,
        }
    }
}

/// Re-open `pid` and confirm it is still the exact process that was probed (pids are reused).
fn reopen(p: &Probe, rights: PROCESS_ACCESS_RIGHTS) -> Option<ProcessHandle> {
    let h = open(p.pid, rights | PROCESS_QUERY_LIMITED_INFORMATION)?;
    let same = handle_path(h.0).map(|x| x.to_lowercase()) == Some(p.key()) && created_at(h.0) == p.created && p.created != 0;
    (same && !critical(h.0)).then_some(h)
}

pub fn file_description(path: &str) -> Option<String> {
    unsafe {
        let wp = wide(path);
        let size = GetFileVersionInfoSizeW(PCWSTR(wp.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(wp.as_ptr()), 0, size, data.as_mut_ptr() as *mut c_void).ok()?;
        let mut p: *mut c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let mut langs: Vec<(u16, u16)> = Vec::new();
        if VerQueryValueW(data.as_ptr() as *const c_void, w!("\\VarFileInfo\\Translation"), &mut p, &mut len).as_bool() && len >= 4 {
            let t = std::slice::from_raw_parts(p as *const u16, (len / 2) as usize);
            for c in t.chunks(2) {
                if c.len() == 2 {
                    langs.push((c[0], c[1]));
                }
            }
        }
        langs.extend([(0x0804, 0x04b0), (0x0409, 0x04b0), (0x0409, 0x04e4)]);
        for (l, cp) in langs {
            let q = wide(&format!("\\StringFileInfo\\{l:04x}{cp:04x}\\FileDescription"));
            if VerQueryValueW(data.as_ptr() as *const c_void, PCWSTR(q.as_ptr()), &mut p, &mut len).as_bool() && len > 1 {
                let s = std::slice::from_raw_parts(p as *const u16, len as usize);
                let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
                let d = String::from_utf16_lossy(&s[..end]).trim().to_string();
                if !d.is_empty() {
                    return Some(d);
                }
            }
        }
        None
    }
}

fn cloaked(h: HWND) -> bool {
    let mut v = 0u32;
    unsafe {
        windows::Win32::Graphics::Dwm::DwmGetWindowAttribute(h, windows::Win32::Graphics::Dwm::DWMWA_CLOAKED, &mut v as *mut _ as *mut c_void, 4).is_ok() && v != 0
    }
}

fn tool_window(h: HWND) -> bool {
    unsafe { (GetWindowLongPtrW(h, GWL_EXSTYLE) as u32) & WS_EX_TOOLWINDOW.0 != 0 }
}

fn unowned(h: HWND) -> bool {
    unsafe { GetWindow(h, GW_OWNER).map(|o| o.is_invalid()).unwrap_or(true) }
}

/// pid -> window title of visible, unowned, titled application windows.
/// Cloaked windows (suspended UWP frames, other virtual desktops, hidden input panels)
/// and tool windows do not make a process a "foreground application".
pub fn visible_windows() -> HashMap<u32, (HWND, String)> {
    unsafe extern "system" fn cb(h: HWND, lp: LPARAM) -> BOOL {
        let map = &mut *(lp.0 as *mut HashMap<u32, (HWND, String)>);
        if IsWindowVisible(h).as_bool() && unowned(h) && !tool_window(h) && !cloaked(h) {
            let mut buf = [0u16; 256];
            let n = GetWindowTextW(h, &mut buf);
            if n > 0 {
                let mut pid = 0u32;
                GetWindowThreadProcessId(h, Some(&mut pid));
                map.entry(pid).or_insert((h, String::from_utf16_lossy(&buf[..n as usize])));
            }
        }
        TRUE
    }
    let mut map: HashMap<u32, (HWND, String)> = HashMap::new();
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut map as *mut _ as isize));
    }
    map
}

/// Processes that still show any real window, including owned dialogs such as
/// "save changes?" prompts and windows on other virtual desktops. Used to decide
/// what must be preserved, so it deliberately errs on the side of keeping.
fn window_pids() -> HashSet<u32> {
    unsafe extern "system" fn cb(h: HWND, lp: LPARAM) -> BOOL {
        let set = &mut *(lp.0 as *mut HashSet<u32>);
        let mut r = RECT::default();
        if IsWindowVisible(h).as_bool() && !tool_window(h) && GetWindowRect(h, &mut r).is_ok() && r.right > r.left && r.bottom > r.top {
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, Some(&mut pid));
            set.insert(pid);
        }
        TRUE
    }
    let mut set = HashSet::new();
    unsafe { let _ = EnumWindows(Some(cb), LPARAM(&mut set as *mut _ as isize)); }
    set
}

fn describe(path: &str) -> Option<String> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(hit) = cache.lock().ok().and_then(|c| c.get(path).cloned()) {
        return hit;
    }
    let value = file_description(path);
    if let Ok(mut c) = cache.lock() {
        if c.len() > 1024 { c.clear(); }
        c.insert(path.to_string(), value.clone());
    }
    value
}

pub fn scan(cfg: &Config) -> Vec<Group> {
    let self_pid = std::process::id();
    let my_session = session_of(self_pid).unwrap_or(u32::MAX);
    let wins = visible_windows();
    let probes = probe_all();
    let mut map: HashMap<String, Group> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut key_of: HashMap<u32, String> = HashMap::new();
    for p in &probes {
        if protected(p, self_pid, my_session) {
            continue;
        }
        let path = p.path.clone().unwrap_or_default();
        let key = path.to_lowercase();
        if cfg.is_whitelisted(&key, &p.name) {
            continue;
        }
        key_of.insert(p.pid, key.clone());
        let g = map.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            let stem = Path::new(&path).file_stem().map(|x| x.to_string_lossy().to_string()).unwrap_or_else(|| p.name.clone());
            Group { key: key.clone(), path: path.clone(), display: describe(&path).unwrap_or(stem), foreground: false, pids: vec![], related: vec![], children: vec![], mem: 0 }
        });
        g.pids.push(p.pid);
        g.mem += p.private;
        if let Some((_, title)) = wins.get(&p.pid) {
            if !g.foreground && g.display.to_lowercase() == core::exe_stem(&p.name) {
                g.display = title.clone();
            }
            g.foreground = true;
        }
    }
    let mut v: Vec<Group> = order.into_iter().filter_map(|k| map.remove(&k)).collect();
    let nodes: Vec<core::Node> = probes.iter().map(Probe::node).collect();
    let tree = core::Tree::new(&nodes);
    let index: HashMap<String, usize> = v.iter().enumerate().map(|(i, g)| (g.key.clone(), i)).collect();
    let mut children: Vec<BTreeSet<String>> = vec![BTreeSet::new(); v.len()];
    let mut related: Vec<BTreeSet<String>> = vec![BTreeSet::new(); v.len()];
    for (i, g) in v.iter().enumerate() {
        // Exactly what "连同子进程" would end: walk only through eligible processes.
        for (pid, _) in tree.descendants(&g.pids, |pid| key_of.contains_key(&pid)) {
            if let Some(&j) = key_of.get(&pid).and_then(|k| index.get(k)) {
                if j != i { children[i].insert(v[j].display.clone()); }
            }
        }
        // A verified foreground ancestor is a useful association hint for background
        // groups, not a claim that other background processes are safe to terminate.
        if g.foreground {
            for (pid, _) in tree.descendants(&g.pids, |_| true) {
                if let Some(&j) = key_of.get(&pid).and_then(|k| index.get(k)) {
                    if j != i && !v[j].foreground { related[j].insert(g.display.clone()); }
                }
            }
        }
    }
    for (i, g) in v.iter_mut().enumerate() {
        g.children = std::mem::take(&mut children[i]).into_iter().collect();
        g.related = std::mem::take(&mut related[i]).into_iter().collect();
    }
    v.sort_by(|a, b| b.mem.cmp(&a.mem).then_with(|| a.display.to_lowercase().cmp(&b.display.to_lowercase())));
    v
}

pub fn mem() -> (u32, u64) {
    let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    unsafe {
        let _ = GlobalMemoryStatusEx(&mut m);
    }
    (m.dwMemoryLoad, m.ullAvailPhys / (1024 * 1024))
}

#[derive(Debug, Default, serde::Serialize)]
pub struct BoostReport {
    pub killed: Vec<String>,
    pub killed_pids: Vec<u32>,
    /// Descendant processes ended together with a selected program.
    pub tree_pids: Vec<u32>,
    pub denied: Vec<String>,
    pub retained: Vec<String>,
    pub touched_pids: Vec<u32>,
    pub graceful_pids: Vec<u32>,
    pub trimmed_pids: Vec<u32>,
    /// Private memory of the processes that ended (MB).
    pub freed_mb: u64,
    /// Working-set reduction of trimmed background processes (MB).
    pub trimmed_mb: u64,
    /// Change of available physical memory, for reference only (MB, may be 0).
    pub avail_delta_mb: u64,
    /// System-wide steps that succeeded (administrator only).
    pub deep: Vec<String>,
    pub deep_needs_admin: bool,
}

struct Candidate {
    process: ProcessHandle,
    pid: u32,
    key: String,
    /// Key of the selected program this process belongs to (itself for selected ones).
    root: String,
    tree: bool,
    can_terminate: bool,
    private: u64,
}

fn running(h: HANDLE) -> bool { unsafe { WaitForSingleObject(h, 0) == WAIT_TIMEOUT } }

fn close_windows(pid: u32) -> bool {
    unsafe extern "system" fn cb(h: HWND, lp: LPARAM) -> BOOL {
        let (wanted, sent) = &mut *(lp.0 as *mut (u32, bool));
        let mut pid = 0;
        GetWindowThreadProcessId(h, Some(&mut pid));
        if pid == *wanted && IsWindowVisible(h).as_bool() && unowned(h) {
            *sent |= PostMessageW(h, WM_CLOSE, WPARAM(0), LPARAM(0)).is_ok();
        }
        TRUE
    }
    let mut context = (pid, false);
    unsafe { let _ = EnumWindows(Some(cb), LPARAM(&mut context as *mut _ as isize)); }
    context.1
}

fn remaining_ms(deadline: Instant) -> u32 {
    deadline.saturating_duration_since(Instant::now()).as_millis() as u32
}

/// Trim one background process: empty its working set and lower its memory priority a
/// little, so Windows repurposes those idle pages before the file cache and the pages of
/// programs in use, instead of them flooding straight back. Returns bytes trimmed.
fn trim_background(p: &Probe) -> Option<u64> {
    let h = reopen(p, PROCESS_SET_QUOTA | PROCESS_SET_INFORMATION).or_else(|| reopen(p, PROCESS_SET_QUOTA))?;
    let before = memory(h.0).1;
    unsafe {
        EmptyWorkingSet(h.0).ok()?;
        let info = MEMORY_PRIORITY_INFORMATION { MemoryPriority: MEMORY_PRIORITY_BELOW_NORMAL };
        let _ = SetProcessInformation(h.0, ProcessMemoryPriority, &info as *const _ as *const c_void, std::mem::size_of::<MEMORY_PRIORITY_INFORMATION>() as u32);
    }
    Some(before.saturating_sub(memory(h.0).1))
}

#[link(name = "ntdll")]
extern "system" {
    fn NtSetSystemInformation(class: i32, info: *const c_void, length: u32) -> i32;
}

fn enable_privilege(name: PCWSTR) -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token).is_err() { return false; }
        let token = ProcessHandle(token);
        let mut luid = LUID::default();
        if LookupPrivilegeValueW(PCWSTR::null(), name, &mut luid).is_err() { return false; }
        let tp = TOKEN_PRIVILEGES { PrivilegeCount: 1, Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }] };
        AdjustTokenPrivileges(token.0, false, Some(&tp), 0, None, None).is_ok() && GetLastError() != ERROR_NOT_ALL_ASSIGNED
    }
}

/// System-wide memory list commands (the same ones RAMMap uses). Administrator only.
fn deep_clean() -> Vec<String> {
    const SYSTEM_MEMORY_LIST_INFORMATION: i32 = 80;
    const STEPS: [(i32, &str); 3] = [(2, "全部进程工作集"), (3, "已修改页"), (5, "低优先级备用页")];
    let _ = enable_privilege(w!("SeIncreaseQuotaPrivilege"));
    if !enable_privilege(w!("SeProfileSingleProcessPrivilege")) { return Vec::new(); }
    STEPS.iter().filter(|(command, _)| unsafe {
        NtSetSystemInformation(SYSTEM_MEMORY_LIST_INFORMATION, command as *const i32 as *const c_void, 4) >= 0
    }).map(|(_, name)| name.to_string()).collect()
}

/// Pin each selected process (and, with `kill_tree`, its verified descendants) by an
/// open handle throughout the operation.
pub fn boost(cfg: &Config, trim: bool) -> BoostReport {
    let mut rep = BoostReport::default();
    let before = mem().1;
    let self_pid = std::process::id();
    let my_session = session_of(self_pid).unwrap_or(u32::MAX);
    let probes = probe_all();
    let by_pid: HashMap<u32, &Probe> = probes.iter().map(|p| (p.pid, p)).collect();
    let eligible = |p: &Probe| !protected(p, self_pid, my_session) && !cfg.is_whitelisted(&p.key(), &p.name);

    let roots: Vec<u32> = probes.iter().filter(|p| eligible(p) && cfg.is_checked(&p.key())).map(|p| p.pid).collect();
    let mut chosen: Vec<(u32, u32, bool)> = roots.iter().map(|&pid| (pid, pid, false)).collect();
    if cfg.kill_tree {
        let nodes: Vec<core::Node> = probes.iter().map(Probe::node).collect();
        let tree = core::Tree::new(&nodes);
        let follow = |pid: u32| by_pid.get(&pid).is_some_and(|p| eligible(p));
        chosen.extend(tree.descendants(&roots, follow).into_iter().map(|(pid, root)| (pid, root, true)));
    }

    let mut candidates = Vec::new();
    for (pid, root, tree) in chosen {
        let (Some(p), Some(r)) = (by_pid.get(&pid), by_pid.get(&root)) else { continue };
        let key = p.key();
        let can_terminate;
        let process = match reopen(p, PROCESS_SYNCHRONIZE | PROCESS_TERMINATE) {
            Some(h) => { can_terminate = true; h }
            None => match reopen(p, PROCESS_SYNCHRONIZE) {
                Some(h) => { can_terminate = false; h }
                None => { if !tree && access_denied(pid) { rep.denied.push(key); } continue; }
            },
        };
        candidates.push(Candidate { process, pid, key, root: r.key(), tree, can_terminate, private: p.private });
    }

    // 1. Ask every windowed program to close normally.
    for c in &candidates {
        if running(c.process.0) && close_windows(c.pid) { rep.graceful_pids.push(c.pid); }
    }
    if !rep.graceful_pids.is_empty() {
        // Wait until the windows are gone (closed, or hidden to the tray) or 5 s passed.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            std::thread::sleep(Duration::from_millis(200));
            let open = window_pids();
            if !candidates.iter().any(|c| running(c.process.0) && open.contains(&c.pid)) || Instant::now() >= deadline { break; }
        }
        // Give programs whose windows closed a moment to finish exiting by themselves.
        let grace = Instant::now() + Duration::from_millis(1500);
        for c in candidates.iter().filter(|c| rep.graceful_pids.contains(&c.pid)) {
            unsafe { WaitForSingleObject(c.process.0, remaining_ms(grace)); }
        }
    }

    // 2. Anything still showing a window (e.g. an unsaved-document prompt) is kept,
    //    together with every process of that program and its tree.
    let open = window_pids();
    let retained: HashSet<String> = candidates.iter().filter(|c| running(c.process.0) && open.contains(&c.pid)).map(|c| c.key.clone()).collect();
    let keep = |c: &Candidate| retained.contains(&c.key) || retained.contains(&c.root);

    // 3. Terminate the rest, parents before children, then wait for all of them together.
    let mut killed = HashSet::new();
    let mut ended: Vec<&Candidate> = Vec::new();
    let mut pending: Vec<&Candidate> = Vec::new();
    for c in &candidates {
        if !running(c.process.0) { ended.push(c); continue; }
        if keep(c) { continue; }
        if !c.can_terminate { rep.denied.push(c.key.clone()); continue; }
        if handle_path(c.process.0).map(|p| p.to_lowercase()) != Some(c.key.clone()) || critical(c.process.0) { continue; }
        rep.touched_pids.push(c.pid);
        match unsafe { TerminateProcess(c.process.0, 1) } {
            Ok(()) => pending.push(c),
            Err(e) if e.code() == E_ACCESSDENIED => rep.denied.push(c.key.clone()),
            Err(_) => {}
        }
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    for c in &pending { unsafe { WaitForSingleObject(c.process.0, remaining_ms(deadline)); } }
    for c in ended.into_iter().chain(pending) {
        killed.insert(c.key.clone());
        rep.killed_pids.push(c.pid);
        if c.tree { rep.tree_pids.push(c.pid); }
        rep.freed_mb += c.private;
    }
    rep.freed_mb /= 1024 * 1024;

    // 4. Optional: trim unselected *background* programs. Foreground applications are
    //    left alone (trimming what you are using only makes it page back in and stutter);
    //    protected, whitelisted and retained programs are never touched.
    if trim {
        let foreground: HashSet<String> = { let w = visible_windows(); probes.iter().filter(|p| w.contains_key(&p.pid)).map(Probe::key).collect() };
        let by_candidate: HashMap<u32, &Candidate> = candidates.iter().map(|c| (c.pid, c)).collect();
        let mut trimmed = 0u64;
        for p in probes.iter().filter(|p| p.pid != self_pid && eligible(p)) {
            let key = p.key();
            if foreground.contains(&key) || retained.contains(&key) { continue; }
            if by_candidate.get(&p.pid).is_some_and(|c| keep(c) || !running(c.process.0)) { continue; }
            if let Some(bytes) = trim_background(p) {
                rep.trimmed_pids.push(p.pid);
                trimmed += bytes;
            }
        }
        rep.trimmed_mb = trimmed / (1024 * 1024);
        if cfg.deep_trim {
            if is_admin() { rep.deep = deep_clean(); } else { rep.deep_needs_admin = true; }
        }
    }

    rep.denied.sort(); rep.denied.dedup();
    for key in retained.iter().chain(rep.denied.iter()) { killed.remove(key); }
    rep.retained = retained.into_iter().collect(); rep.retained.sort();
    rep.killed = killed.into_iter().collect(); rep.killed.sort();
    std::thread::sleep(Duration::from_millis(300));
    rep.avail_delta_mb = mem().1.saturating_sub(before);
    rep
}

pub fn is_admin() -> bool {
    unsafe { IsUserAnAdmin().as_bool() }
}

pub fn shell_open(file: &str, args: Option<&str>, verb: Option<&str>) -> bool {
    shell_open_at(file, args, verb, None)
}

fn shell_open_at(file: &str, args: Option<&str>, verb: Option<&str>, directory: Option<&str>) -> bool {
    let _com = ComScope::new();
    let f = wide(file);
    let a = args.map(wide);
    let v = verb.map(wide);
    let d = directory.map(wide);
    unsafe {
        let r = ShellExecuteW(
            None,
            v.as_ref().map(|v| PCWSTR(v.as_ptr())).unwrap_or(PCWSTR::null()),
            PCWSTR(f.as_ptr()),
            a.as_ref().map(|a| PCWSTR(a.as_ptr())).unwrap_or(PCWSTR::null()),
            d.as_ref().map(|d| PCWSTR(d.as_ptr())).unwrap_or(PCWSTR::null()),
            SW_SHOWNORMAL,
        );
        r.0 as isize > 32
    }
}

// ---------- autostart ----------
const RUN: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

pub fn autostart_get() -> bool {
    let name = wide(core::RUN_VALUE);
    unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN, PCWSTR(name.as_ptr()), RRF_RT_REG_SZ, None, None, None).is_ok() }
}

pub fn autostart_set(on: bool) -> bool {
    let name = wide(core::RUN_VALUE);
    unsafe {
        let mut key = HKEY::default();
        if RegCreateKeyExW(HKEY_CURRENT_USER, RUN, 0, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, None, &mut key, None).is_err() {
            return false;
        }
        let r = if on {
            let exe = std::env::current_exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
            let val = wide(&format!("\"{exe}\" --autostart"));
            let bytes = std::slice::from_raw_parts(val.as_ptr() as *const u8, val.len() * 2);
            RegSetValueExW(key, PCWSTR(name.as_ptr()), 0, REG_SZ, Some(bytes)).is_ok()
        } else {
            let e = RegDeleteValueW(key, PCWSTR(name.as_ptr()));
            e.is_ok() || e == ERROR_FILE_NOT_FOUND
        };
        let _ = RegCloseKey(key);
        r
    }
}

// ---------- icons ----------
/// RGBA pixels of a file's (or shortcut's) shell icon.
pub fn file_icon(path: &str, large: bool) -> Option<(u32, u32, Vec<u8>)> {
    unsafe {
        let wp = wide(path);
        let mut info = SHFILEINFOW::default();
        let flags = SHGFI_ICON | if large { SHGFI_LARGEICON } else { SHGFI_SMALLICON };
        let r = SHGetFileInfoW(PCWSTR(wp.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(0), Some(&mut info), std::mem::size_of::<SHFILEINFOW>() as u32, flags);
        if r == 0 || info.hIcon.is_invalid() {
            return None;
        }
        let px = hicon_rgba(info.hIcon);
        let _ = DestroyIcon(info.hIcon);
        px
    }
}

unsafe fn hicon_rgba(icon: HICON) -> Option<(u32, u32, Vec<u8>)> {
    let mut ii = ICONINFO::default();
    GetIconInfo(icon, &mut ii).ok()?;
    let mut bm = BITMAP::default();
    let color = ii.hbmColor;
    let ok = GetObjectW(color, std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut c_void)) != 0;
    let res = if ok && bm.bmWidth > 0 {
        let (w, h) = (bm.bmWidth, bm.bmHeight);
        let mut bi = BITMAPINFO { bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() }, ..Default::default() };
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let dc = GetDC(None);
        let n = GetDIBits(dc, color, 0, h as u32, Some(buf.as_mut_ptr() as *mut c_void), &mut bi, DIB_RGB_COLORS);
        ReleaseDC(None, dc);
        if n == 0 {
            None
        } else {
            let has_alpha = buf.chunks(4).any(|p| p[3] != 0);
            for p in buf.chunks_mut(4) {
                p.swap(0, 2);
                if !has_alpha { p[3] = 255; }
            }
            Some((w as u32, h as u32, buf))
        }
    } else {
        None
    };
    if !ii.hbmColor.is_invalid() { let _ = DeleteObject(ii.hbmColor); }
    if !ii.hbmMask.is_invalid() { let _ = DeleteObject(ii.hbmMask); }
    res
}

// ---------- shortcuts ----------
struct ComScope(bool);
impl ComScope {
    fn new() -> Self {
        Self(unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).is_ok() })
    }
}
impl Drop for ComScope {
    fn drop(&mut self) { if self.0 { unsafe { CoUninitialize(); } } }
}

fn shortcut_program(path: &Path) -> bool {
    let _com = ComScope::new();
    let read = || -> Option<String> {
        unsafe {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
            let file: IPersistFile = link.cast().ok()?;
            let name = wide(&path.to_string_lossy());
            file.Load(PCWSTR(name.as_ptr()), STGM_READ).ok()?;
            let mut target = [0u16; 32768];
            // Reading the stored target does not search, repair or run a link.
            link.GetPath(&mut target, std::ptr::null_mut(), 0).ok()?;
            let end = target.iter().position(|&c| c == 0)?;
            Some(String::from_utf16_lossy(&target[..end]))
        }
    };
    read().is_some_and(|target| core::valid_launcher_file(&target) && Path::new(&target).extension().is_some_and(|ext| ext.eq_ignore_ascii_case("exe")))
}

fn known(id: &windows::core::GUID) -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const c_void));
        s.map(PathBuf::from)
    }
}

pub fn scan_launchers() -> Vec<Launcher> {
    let dirs = [
        known(&FOLDERID_CommonPrograms),
        known(&FOLDERID_Programs),
        known(&FOLDERID_Desktop),
        known(&FOLDERID_PublicDesktop),
    ];
    let mut out = Vec::new();
    for d in dirs.into_iter().flatten() {
        walk(&d, 0, &mut out);
    }
    core::clean_launchers(out)
}

fn walk(d: &Path, depth: u32, out: &mut Vec<Launcher>) {
    if depth > 3 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(d) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, depth + 1, out);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")) && shortcut_program(&p) {
            if let Some(n) = p.file_stem() {
                out.push(Launcher { name: n.to_string_lossy().to_string(), path: p.to_string_lossy().to_string() });
            }
        }
    }
}

pub fn choose_shortcut(owner: Option<HWND>) -> Option<Launcher> {
    let mut buffer = [0u16; 32768];
    let filter = wide("快捷方式和程序 (*.lnk;*.exe)\0*.lnk;*.exe\0");
    let title = wide("添加快捷方式");
    let mut dialog = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner.unwrap_or_default(),
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(buffer.as_mut_ptr()),
        nMaxFile: buffer.len() as u32,
        lpstrTitle: PCWSTR(title.as_ptr()),
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | OFN_NODEREFERENCELINKS | OFN_DONTADDTORECENT | OFN_EXPLORER,
        ..Default::default()
    };
    if !unsafe { GetOpenFileNameW(&mut dialog) }.as_bool() { return None; }
    let end = buffer.iter().position(|&c| c == 0)?;
    let path = String::from_utf16_lossy(&buffer[..end]);
    if !core::valid_launcher_file(&path) { return None; }
    let name = Path::new(&path).file_stem()?.to_string_lossy().to_string();
    Some(Launcher { name, path })
}

pub fn launch_shortcut(item: &Launcher) -> bool {
    // Let the shell open the .lnk itself, preserving its arguments and cwd.
    if !core::valid_launcher_file(&item.path) { return false; }
    let path = Path::new(&item.path);
    let directory = if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("exe")) {
        path.parent().map(|p| p.to_string_lossy().into_owned())
    } else { None };
    shell_open_at(&item.path, None, Some("open"), directory.as_deref())
}

pub fn system_exe(name: &str) -> String {
    let root = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let root = PathBuf::from(root);
    let path = if name.eq_ignore_ascii_case("explorer.exe") { root.join(name) } else { root.join("System32").join(name) };
    path.to_string_lossy().to_string()
}

// ---------- window helpers ----------
pub fn find_own_window(title: &str) -> Option<HWND> {
    let t = wide(title);
    let me = std::process::id();
    let mut cur: Option<HWND> = None;
    loop {
        let h = unsafe { FindWindowExW(None, cur.unwrap_or_default(), PCWSTR::null(), PCWSTR(t.as_ptr())) }.ok()?;
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
        if pid == me {
            return Some(h);
        }
        cur = Some(h);
    }
}

pub fn style_panel(h: HWND) {
    unsafe {
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        SetWindowLongPtrW(h, GWL_EXSTYLE, (ex | WS_EX_TOOLWINDOW.0 as isize) & !(WS_EX_APPWINDOW.0 as isize));
        let dark: BOOL = TRUE;
        let _ = windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(h, windows::Win32::Graphics::Dwm::DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const c_void, 4);
        let round = windows::Win32::Graphics::Dwm::DWMWCP_ROUND;
        let _ = windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(h, windows::Win32::Graphics::Dwm::DWMWA_WINDOW_CORNER_PREFERENCE, &round as *const _ as *const c_void, 4);
    }
}

pub fn foreground_is(h: HWND) -> bool {
    unsafe {
        let mut active = GetForegroundWindow();
        for _ in 0..16 {
            if active == h { return true; }
            let Ok(owner) = GetWindow(active, GW_OWNER) else { return false };
            if owner.is_invalid() { return false; }
            active = owner;
        }
        false
    }
}

pub fn bring_to_front(h: HWND) {
    unsafe {
        let _ = SetWindowPos(h, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW);
        let _ = SetForegroundWindow(h);
    }
}

fn rect(r: RECT) -> core::Rect {
    core::Rect { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
}

/// Physical position for a (w,h) physical-px panel next to the tray icon.
pub fn panel_pos(w: i32, h: i32) -> (i32, i32) {
    unsafe {
        let mut work = RECT::default();
        let _ = SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut work as *mut _ as *mut c_void), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
        let screen = RECT { left: 0, top: 0, right: GetSystemMetrics(SM_CXSCREEN), bottom: GetSystemMetrics(SM_CYSCREEN) };
        let anchor = tray::icon_rect().map(rect);
        core::panel_position(rect(work), rect(screen), anchor, w, h)
    }
}

pub fn free_console() {
    unsafe {
        let _ = windows::Win32::System::Console::FreeConsole();
    }
}

// ---------- tray ----------
pub mod tray {
    use super::*;
    use std::cell::RefCell;

    pub enum Event { Left, Open, Settings, Quit }
    const WM_TRAY: u32 = WM_APP + 1;
    const ID_OPEN: usize = 1;
    const ID_SETTINGS: usize = 2;
    const ID_QUIT: usize = 3;

    type Handler = RefCell<Option<Box<dyn Fn(Event)>>>;
    thread_local! {
        static HANDLER: Handler = const { RefCell::new(None) };
        static HWND_: RefCell<HWND> = RefCell::new(HWND::default());
    }

    fn emit(e: Event) {
        HANDLER.with(|h| if let Some(f) = h.borrow().as_ref() { f(e) });
    }

    unsafe extern "system" fn proc_(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if msg == WM_TRAY {
            match (lp.0 as u32) & 0xffff {
                WM_LBUTTONUP => emit(Event::Left),
                WM_RBUTTONUP | WM_CONTEXTMENU => {
                    if let Ok(m) = CreatePopupMenu() {
                        let _ = AppendMenuW(m, MF_STRING, ID_OPEN, w!("打开面板"));
                        let _ = AppendMenuW(m, MF_STRING, ID_SETTINGS, w!("设置"));
                        let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
                        let _ = AppendMenuW(m, MF_STRING, ID_QUIT, w!("退出"));
                        let mut pt = POINT::default();
                        let _ = GetCursorPos(&mut pt);
                        let _ = SetForegroundWindow(h);
                        let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, 0, h, None);
                        let _ = DestroyMenu(m);
                        match cmd.0 as usize {
                            ID_OPEN => emit(Event::Open),
                            ID_SETTINGS => emit(Event::Settings),
                            ID_QUIT => emit(Event::Quit),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            return LRESULT(0);
        }
        DefWindowProcW(h, msg, wp, lp)
    }

    fn nid(h: HWND) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: h, uID: 1, ..Default::default() }
    }

    /// Create the tray icon. Must run on the UI thread (winit pumps its messages).
    pub fn install(tip: &str, handler: impl Fn(Event) + 'static) -> bool {
        HANDLER.with(|h| *h.borrow_mut() = Some(Box::new(handler)));
        unsafe {
            let inst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap_or_default();
            let wc = WNDCLASSW { lpfnWndProc: Some(proc_), hInstance: inst.into(), lpszClassName: w!("BlitzTray"), ..Default::default() };
            RegisterClassW(&wc);
            let Ok(h) = CreateWindowExW(WINDOW_EX_STYLE(0), w!("BlitzTray"), w!("BlitzTray"), WINDOW_STYLE(0), 0, 0, 0, 0, HWND_MESSAGE, None, inst, None) else { return false };
            HWND_.with(|x| *x.borrow_mut() = h);
            let mut n = nid(h);
            n.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
            n.uCallbackMessage = WM_TRAY;
            n.hIcon = LoadImageW(inst, PCWSTR(std::ptr::without_provenance(1)), IMAGE_ICON, 0, 0, LR_DEFAULTSIZE | LR_SHARED)
                .map(|x| HICON(x.0))
                .or_else(|_| LoadIconW(None, IDI_APPLICATION))
                .unwrap_or_default();
            let t = wide(tip);
            let l = t.len().min(n.szTip.len());
            n.szTip[..l].copy_from_slice(&t[..l]);
            Shell_NotifyIconW(NIM_ADD, &n).as_bool()
        }
    }

    pub fn remove() {
        let h = HWND_.with(|x| *x.borrow());
        unsafe { let _ = Shell_NotifyIconW(NIM_DELETE, &nid(h)); }
    }

    pub fn icon_rect() -> Option<RECT> {
        let h = HWND_.with(|x| *x.borrow());
        if h.is_invalid() { return None; }
        let id = NOTIFYICONIDENTIFIER { cbSize: std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32, hWnd: h, uID: 1, ..Default::default() };
        unsafe { Shell_NotifyIconGetRect(&id).ok() }
    }
}

/// False when another Blitz UI instance is already running in this session.
pub fn single_instance(wait: bool) -> bool {
    unsafe {
        match windows::Win32::System::Threading::CreateMutexW(None, true, w!("Local\\BlitzSingleInstance")) {
            Ok(h) => {
                let first = GetLastError() != ERROR_ALREADY_EXISTS;
                let acquired = first || (wait && matches!(WaitForSingleObject(h, 10000), WAIT_OBJECT_0 | WAIT_ABANDONED));
                let _ = h; // keep the mutex handle open for the process lifetime
                acquired
            }
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn system_entries_resolve_to_real_absolute_windows_files() {
        for name in ["explorer.exe", "shutdown.exe"] {
            let file = super::system_exe(name);
            assert!(std::path::Path::new(&file).is_absolute());
            assert!(std::path::Path::new(&file).is_file(), "{file}");
            assert!(super::windows_path(&file));
        }
    }
}
