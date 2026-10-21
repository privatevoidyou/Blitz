//! Win32 layer: processes, kill, memory, icons, shortcuts, autostart, tray.
#![cfg(windows)]
use crate::core::{self, Config, Group, Launcher, ProcFacts};
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Diagnostics::ToolHelp::*;
use windows::Win32::System::ProcessStatus::EmptyWorkingSet;
use windows::Win32::System::Registry::*;
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Shell::*;
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

pub fn image_path(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut n = buf.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut n);
        let _ = CloseHandle(h);
        r.ok().map(|_| String::from_utf16_lossy(&buf[..n as usize]))
    }
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

/// pid -> window title of visible, unowned top-level windows.
pub fn visible_windows() -> HashMap<u32, (HWND, String)> {
    unsafe extern "system" fn cb(h: HWND, lp: LPARAM) -> BOOL {
        let map = &mut *(lp.0 as *mut HashMap<u32, (HWND, String)>);
        if IsWindowVisible(h).as_bool() && GetWindow(h, GW_OWNER).map(|o| o.is_invalid()).unwrap_or(true) {
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

fn facts_protected(s: &Snap, path: Option<&str>, self_pid: u32, my_session: u32) -> bool {
    core::is_protected(&ProcFacts { pid: s.pid, exe_name: &s.name, session: session_of(s.pid), path }, self_pid, my_session)
}

pub fn scan(cfg: &Config) -> Vec<Group> {
    let self_pid = std::process::id();
    let my_session = session_of(self_pid).unwrap_or(u32::MAX);
    let wins = visible_windows();
    let mut map: HashMap<String, Group> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for s in snapshot() {
        let path = image_path(s.pid);
        if facts_protected(&s, path.as_deref(), self_pid, my_session) {
            continue;
        }
        let path = path.unwrap_or_default();
        let key = path.to_lowercase();
        if cfg.is_whitelisted(&key, &s.name) {
            continue;
        }
        let g = map.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            let stem = Path::new(&path).file_stem().map(|x| x.to_string_lossy().to_string()).unwrap_or_else(|| s.name.clone());
            Group { key: key.clone(), path: path.clone(), display: file_description(&path).unwrap_or(stem), foreground: false, pids: vec![] }
        });
        g.pids.push(s.pid);
        if let Some((_, title)) = wins.get(&s.pid) {
            if !g.foreground && g.display.to_lowercase() == core::exe_stem(&s.name) {
                g.display = title.clone();
            }
            g.foreground = true;
        }
    }
    let mut v: Vec<Group> = order.into_iter().filter_map(|k| map.remove(&k)).collect();
    v.sort_by_key(|g| g.display.to_lowercase());
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
    pub denied: Vec<String>,
    pub touched_pids: Vec<u32>,
    pub freed_mb: u64,
}

fn terminate(pid: u32) -> Result<(), bool> {
    unsafe {
        match OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, pid) {
            Ok(h) => {
                let r = TerminateProcess(h, 1);
                if r.is_ok() {
                    WaitForSingleObject(h, 3000);
                }
                let _ = CloseHandle(h);
                r.map_err(|e| e.code() == E_ACCESSDENIED)
            }
            Err(e) => Err(e.code() == E_ACCESSDENIED),
        }
    }
}

fn alive(pid: u32) -> bool {
    unsafe {
        match OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            Ok(h) => {
                let r = WaitForSingleObject(h, 0) == WAIT_TIMEOUT;
                let _ = CloseHandle(h);
                r
            }
            Err(_) => false,
        }
    }
}

/// Close then kill only the checked groups. Every PID is re-verified (same exe path, not protected).
pub fn boost(cfg: &Config, trim: bool) -> BoostReport {
    let mut rep = BoostReport::default();
    let before = mem().1;
    let self_pid = std::process::id();
    let my_session = session_of(self_pid).unwrap_or(u32::MAX);
    let snaps = snapshot();
    let mut targets: Vec<(u32, String)> = Vec::new(); // (pid, key)
    for s in &snaps {
        let path = image_path(s.pid);
        if facts_protected(s, path.as_deref(), self_pid, my_session) {
            continue;
        }
        let key = path.unwrap_or_default().to_lowercase();
        if cfg.is_checked(&key) && !cfg.is_whitelisted(&key, &s.name) {
            targets.push((s.pid, key));
        }
    }
    // descendants of each target (process tree), excluding protected ones
    let mut children: HashMap<u32, Vec<&Snap>> = HashMap::new();
    for s in &snaps {
        children.entry(s.parent).or_default().push(s);
    }
    let wins = visible_windows();
    let mut asked = false;
    for (pid, _) in &targets {
        if let Some((h, _)) = wins.get(pid) {
            unsafe {
                let _ = PostMessageW(*h, WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            asked = true;
        }
    }
    if asked {
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
    let mut seen = HashSet::new();
    let mut killed_keys = HashSet::new();
    let mut denied_keys = HashSet::new();
    for (pid, key) in &targets {
        // tree, children first
        let mut stack = vec![*pid];
        let mut tree = Vec::new();
        while let Some(p) = stack.pop() {
            if !seen.insert(p) {
                continue;
            }
            tree.push(p);
            if let Some(cs) = children.get(&p) {
                for c in cs {
                    if c.pid != p && !facts_protected(c, image_path(c.pid).as_deref(), self_pid, my_session) {
                        stack.push(c.pid);
                    }
                }
            }
        }
        for p in tree.into_iter().rev() {
            if p == *pid && image_path(p).map(|x| x.to_lowercase()) != Some(key.clone()) {
                continue; // PID reused or already gone
            }
            if !alive(p) {
                if p == *pid { killed_keys.insert(key.clone()); rep.killed_pids.push(p); }
                continue;
            }
            rep.touched_pids.push(p);
            match terminate(p) {
                Ok(()) => {
                    if p == *pid { killed_keys.insert(key.clone()); }
                    rep.killed_pids.push(p);
                }
                Err(true) => { denied_keys.insert(key.clone()); }
                Err(false) => {}
            }
        }
    }
    if trim {
        for s in snapshot() {
            unsafe {
                if let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SET_QUOTA, false, s.pid) {
                    let _ = EmptyWorkingSet(h);
                    let _ = CloseHandle(h);
                }
            }
        }
    }
    for k in &denied_keys { killed_keys.remove(k); }
    rep.killed = killed_keys.into_iter().collect();
    rep.denied = denied_keys.into_iter().collect();
    std::thread::sleep(std::time::Duration::from_millis(300));
    rep.freed_mb = mem().1.saturating_sub(before);
    rep
}

pub fn is_admin() -> bool {
    unsafe { IsUserAnAdmin().as_bool() }
}

pub fn shell_open(file: &str, args: Option<&str>, verb: Option<&str>) -> bool {
    let f = wide(file);
    let a = args.map(wide);
    let v = verb.map(wide);
    unsafe {
        let r = ShellExecuteW(
            None,
            v.as_ref().map(|v| PCWSTR(v.as_ptr())).unwrap_or(PCWSTR::null()),
            PCWSTR(f.as_ptr()),
            a.as_ref().map(|a| PCWSTR(a.as_ptr())).unwrap_or(PCWSTR::null()),
            PCWSTR::null(),
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
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk") || x.eq_ignore_ascii_case("url")) {
            if let Some(n) = p.file_stem() {
                out.push(Launcher { name: n.to_string_lossy().to_string(), path: p.to_string_lossy().to_string() });
            }
        }
    }
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
    unsafe { GetForegroundWindow() == h }
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
