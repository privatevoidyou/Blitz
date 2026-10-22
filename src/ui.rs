//! Tray + panel UI glue.
use crate::core::{self, Config, Group, Launcher, Row};
use crate::win;
use slint::{ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

slint::include_modules!();

#[cfg(test)]
#[path = "ui_tests.rs"]
mod tests;

const PER_PAGE: usize = 12;

struct State {
    cfg: Config,
    dir: PathBuf,
    groups: Vec<Group>,
    rows: Vec<Row>,
    proc_icons: HashMap<String, Image>,
    launchers: Vec<Launcher>,
    launcher_icons: Vec<Option<Image>>,
    page: usize,
    page_items: Vec<usize>,
    shown_at: Option<Instant>,
    keep_open: bool,
}

impl State {
    fn save(&self) {
        let _ = self.cfg.save(&self.dir);
    }
}

type S = Rc<RefCell<State>>;

fn to_image(w: u32, h: u32, px: &[u8]) -> Image {
    Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(px, w, h))
}

fn refresh_procs(ui: &Panel, st: &S) {
    let mut s = st.borrow_mut();
    s.groups = win::scan(&s.cfg);
    let paths: Vec<String> = s.groups.iter().map(|g| g.path.clone()).collect();
    for p in paths {
        s.proc_icons.entry(p).or_insert_with_key(|p| win::file_icon(p, false).map(|(w, h, px)| to_image(w, h, &px)).unwrap_or_default());
    }
    render_rows(ui, &mut s);
}

fn render_rows(ui: &Panel, s: &mut State) {
    s.rows = core::build_rows(&s.groups, &s.cfg, ui.get_proc_query().as_str());
    let v: Vec<Proc> = s
        .rows
        .iter()
        .map(|r| Proc {
            label: r.label.clone().into(),
            icon: r.group.and_then(|i| s.proc_icons.get(&s.groups[i].path).cloned()).unwrap_or_default(),
            checked: r.checked,
            header: r.group.is_none(),
        })
        .collect();
    ui.set_items(ModelRc::new(VecModel::from(v)));
}

fn render_apps(ui: &Panel, s: &mut State) {
    let q = ui.get_app_query().to_string();
    let (_, pages) = core::page_of(&s.launchers, &q, s.page, PER_PAGE);
    s.page = s.page.min(pages - 1);
    let ql = q.trim().to_lowercase();
    let filtered: Vec<usize> = (0..s.launchers.len())
        .filter(|&i| ql.is_empty() || s.launchers[i].name.to_lowercase().contains(&ql))
        .collect();
    s.page_items = filtered.into_iter().skip(s.page * PER_PAGE).take(PER_PAGE).collect();
    // icons are extracted lazily, only for the visible page, and cached
    for k in s.page_items.clone() {
        if s.launcher_icons[k].is_none() {
            let img = win::file_icon(&s.launchers[k].path, true).map(|(w, h, px)| to_image(w, h, &px)).unwrap_or_default();
            s.launcher_icons[k] = Some(img);
        }
    }
    let v: Vec<App> = s
        .page_items
        .iter()
        .map(|&i| App { name: s.launchers[i].name.clone().into(), icon: s.launcher_icons.get(i).cloned().flatten().unwrap_or_default() })
        .collect();
    ui.set_apps(ModelRc::new(VecModel::from(v)));
    ui.set_page(s.page as i32);
    ui.set_pages(pages as i32);
}

fn update_mem(ui: &Panel) {
    ui.set_mem_pct(win::mem().0 as f32 / 100.0);
}

fn show_panel(ui: &Panel, st: &S) {
    update_mem(ui);
    refresh_procs(ui, st);
    let size = ui.window().size();
    let (x, y) = win::panel_pos(size.width as i32, size.height as i32);
    ui.window().set_position(slint::PhysicalPosition::new(x, y));
    let _ = ui.show();
    if let Some(h) = win::find_own_window("疾风加速") {
        win::style_panel(h);
        win::bring_to_front(h);
    }
    st.borrow_mut().shown_at = Some(Instant::now());
}

fn hide_panel(ui: &Panel, st: &S) {
    let _ = ui.hide();
    st.borrow_mut().shown_at = None;
}

fn start_boost(ui: &Panel, st: &S) {
    if ui.get_busy() {
        return;
    }
    ui.set_busy(true);
    ui.set_status("正在加速…".into());
    ui.set_show_admin(false);
    let cfg = st.borrow().cfg.clone();
    let dir = st.borrow().dir.clone();
    let trim = ui.get_trim();
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let rep = win::boost(&cfg, trim);
        let _ = std::fs::write(dir.join("last_boost.json"), serde_json::to_string_pretty(&rep).unwrap_or_default());
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak.upgrade() {
                let mut msg = format!("已结束 {} 个程序，释放内存约 {} MB", rep.killed.len(), rep.freed_mb);
                if !rep.denied.is_empty() {
                    msg += &format!("；{} 个需管理员权限", rep.denied.len());
                    ui.set_show_admin(!win::is_admin());
                }
                ui.set_status(msg.into());
                ui.set_busy(false);
                update_mem(&ui);
                ui.invoke_proc_search(ui.get_proc_query());
            }
        });
    });
}

pub fn run(args: Vec<String>, dir: PathBuf, legacy: Option<PathBuf>) {
    let cfg = Config::load(&dir, legacy.as_deref());
    let has = |f: &str| args.iter().any(|a| a == f);
    let ui = Panel::new().expect("ui");
    let st: S = Rc::new(RefCell::new(State {
        cfg: cfg.clone(),
        dir,
        groups: vec![],
        rows: vec![],
        proc_icons: HashMap::new(),
        launchers: vec![],
        launcher_icons: vec![],
        page: 0,
        page_items: vec![],
        shown_at: None,
        keep_open: has("--keep-open"),
    }));
    // persist migrated config under the new path
    st.borrow().save();

    ui.set_trim(cfg.trim_memory);
    ui.set_trim_default(cfg.trim_memory);
    ui.set_auto_boost(cfg.auto_boost);
    ui.set_auto_start(win::autostart_get());

    // ---- launchers ----
    st.borrow_mut().launchers = win::scan_launchers();
    {
        let mut s = st.borrow_mut();
        s.launcher_icons = vec![None; s.launchers.len()];
        render_apps(&ui, &mut s);
    }

    // ---- callbacks ----
    let w = ui.as_weak();
    let s2 = st.clone();
    ui.on_app_search(move |_| {
        if let Some(ui) = w.upgrade() {
            let mut s = s2.borrow_mut();
            s.page = 0;
            render_apps(&ui, &mut s);
        }
    });
    let w = ui.as_weak();
    let s2 = st.clone();
    ui.on_set_page(move |p| {
        if let Some(ui) = w.upgrade() {
            let mut s = s2.borrow_mut();
            s.page = p.max(0) as usize;
            render_apps(&ui, &mut s);
        }
    });
    let w = ui.as_weak();
    let s2 = st.clone();
    ui.on_launch(move |i| {
        let path = s2.borrow().page_items.get(i as usize).map(|&k| s2.borrow().launchers[k].path.clone());
        if let (Some(p), Some(ui)) = (path, w.upgrade()) {
            win::shell_open(&p, None, None);
            if !s2.borrow().keep_open {
                hide_panel(&ui, &s2);
            }
        }
    });
    let w = ui.as_weak();
    let s2 = st.clone();
    ui.on_proc_search(move |_| {
        if let Some(ui) = w.upgrade() {
            refresh_procs(&ui, &s2);
        }
    });
    let w = ui.as_weak();
    let s2 = st.clone();
    ui.on_show_procs(move || {
        if let Some(ui) = w.upgrade() {
            ui.set_view(1);
            refresh_procs(&ui, &s2);
        }
    });
    let w = ui.as_weak();
    let s2 = st.clone();
    ui.on_toggled(move |i, on| {
        let mut s = s2.borrow_mut();
        let Some(gi) = s.rows.get(i as usize).and_then(|r| r.group) else { return };
        let key = s.groups[gi].key.clone();
        s.cfg.set_checked(&key, on);
        s.rows[i as usize].checked = on;
        s.save();
        if let Some(ui) = w.upgrade() {
            let m = ui.get_items();
            if let Some(mut row) = m.row_data(i as usize) {
                row.checked = on;
                m.set_row_data(i as usize, row);
            }
        }
    });
    let w = ui.as_weak();
    let s2 = st.clone();
    ui.on_whitelist(move |i| {
        {
            let mut s = s2.borrow_mut();
            let Some(gi) = s.rows.get(i as usize).and_then(|r| r.group) else { return };
            let key = s.groups[gi].key.clone();
            s.cfg.whitelist.insert(key.clone());
            s.cfg.set_checked(&key, false);
            s.save();
        }
        if let Some(ui) = w.upgrade() {
            refresh_procs(&ui, &s2);
        }
    });
    let w = ui.as_weak();
    let s2 = st.clone();
    ui.on_boost(move || {
        if let Some(ui) = w.upgrade() {
            start_boost(&ui, &s2);
        }
    });
    let w = ui.as_weak();
    let s2 = st.clone();
    ui.on_trim_changed(move |on| {
        let mut s = s2.borrow_mut();
        s.cfg.trim_memory = on;
        s.save();
        if let Some(ui) = w.upgrade() {
            ui.set_trim_default(on);
        }
    });
    let w = ui.as_weak();
    let s2 = st.clone();
    ui.on_trim_default_changed(move |on| {
        let mut s = s2.borrow_mut();
        s.cfg.trim_memory = on;
        s.save();
        if let Some(ui) = w.upgrade() {
            ui.set_trim(on);
        }
    });
    let s2 = st.clone();
    ui.on_auto_boost_changed(move |on| {
        let mut s = s2.borrow_mut();
        s.cfg.auto_boost = on;
        s.save();
    });
    let w = ui.as_weak();
    ui.on_auto_start_changed(move |on| {
        let ok = win::autostart_set(on);
        if let Some(ui) = w.upgrade() {
            ui.set_auto_start(win::autostart_get());
            if !ok {
                ui.set_status("无法写入开机启动项".into());
            }
        }
    });
    ui.on_restart_admin(|| {
        if let Ok(exe) = std::env::current_exe() {
            if win::shell_open(&exe.to_string_lossy(), Some("--open-panel"), Some("runas")) {
                win::tray::remove();
                std::process::exit(0);
            }
        }
    });
    let w = ui.as_weak();
    let armed_timer = Rc::new(slint::Timer::default());
    ui.on_shutdown(move || {
        let Some(ui) = w.upgrade() else { return };
        if ui.get_shutdown_armed() {
            ui.set_shutdown_armed(false);
            win::shell_open("shutdown.exe", Some("/s /t 0"), None);
        } else {
            ui.set_shutdown_armed(true);
            let w2 = ui.as_weak();
            armed_timer.start(slint::TimerMode::SingleShot, Duration::from_secs(3), move || {
                if let Some(ui) = w2.upgrade() {
                    ui.set_shutdown_armed(false);
                }
            });
        }
    });
    let w = ui.as_weak();
    let s2 = st.clone();
    ui.on_my_computer(move || {
        win::shell_open("explorer.exe", Some("shell:MyComputerFolder"), None);
        if let Some(ui) = w.upgrade() {
            if !s2.borrow().keep_open {
                hide_panel(&ui, &s2);
            }
        }
    });
    let w = ui.as_weak();
    let s2 = st.clone();
    ui.on_screenshot(move || {
        if let Some(ui) = w.upgrade() {
            hide_panel(&ui, &s2);
        }
        slint::Timer::single_shot(Duration::from_millis(250), || {
            win::shell_open("ms-screenclip:", None, None);
        });
    });

    // ---- tray ----
    let w = ui.as_weak();
    let s2 = st.clone();
    win::tray::install("Blitz 疾风加速", move |e| {
        let w = w.clone();
        let s2 = s2.clone();
        slint::Timer::single_shot(Duration::from_millis(0), move || {
            let Some(ui) = w.upgrade() else { return };
            match e {
                win::tray::Event::Left => {
                    if ui.window().is_visible() {
                        hide_panel(&ui, &s2);
                    } else {
                        ui.set_view(0);
                        show_panel(&ui, &s2);
                    }
                }
                win::tray::Event::Open => {
                    ui.set_view(0);
                    show_panel(&ui, &s2);
                }
                win::tray::Event::Settings => {
                    ui.set_view(2);
                    show_panel(&ui, &s2);
                }
                win::tray::Event::Quit => {
                    win::tray::remove();
                    let _ = slint::quit_event_loop();
                }
            }
        });
    });

    // ---- periodic: memory + auto-hide on focus loss ----
    let w = ui.as_weak();
    let s2 = st.clone();
    let tick = slint::Timer::default();
    let mut n = 0u32;
    tick.start(slint::TimerMode::Repeated, Duration::from_millis(250), move || {
        let Some(ui) = w.upgrade() else { return };
        if !ui.window().is_visible() {
            return;
        }
        n += 1;
        if n.is_multiple_of(8) {
            update_mem(&ui);
        }
        let (shown, keep) = { let s = s2.borrow(); (s.shown_at, s.keep_open) };
        if keep {
            return;
        }
        if let (Some(t), Some(h)) = (shown, win::find_own_window("疾风加速")) {
            if t.elapsed() > Duration::from_millis(600) && !win::foreground_is(h) {
                hide_panel(&ui, &s2);
            }
        }
    });

    // ---- startup actions ----
    if cfg.auto_boost && !cfg.checked.is_empty() {
        start_boost(&ui, &st);
    }
    if has("--open-panel") || arg_view(&args).is_some() {
        let view = arg_view(&args).unwrap_or(0);
        let w = ui.as_weak();
        let s2 = st.clone();
        slint::Timer::single_shot(Duration::from_millis(300), move || {
            if let Some(ui) = w.upgrade() {
                ui.set_view(view);
                show_panel(&ui, &s2);
            }
        });
    }
    let _ = slint::run_event_loop_until_quit();
    win::tray::remove();
}

fn arg_view(args: &[String]) -> Option<i32> {
    let i = args.iter().position(|a| a == "--view")?;
    match args.get(i + 1).map(String::as_str) {
        Some("procs") => Some(1),
        Some("settings") => Some(2),
        _ => Some(0),
    }
}
