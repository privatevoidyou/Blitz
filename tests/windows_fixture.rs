//! A native Windows test process. It never operates on user files/processes.
#![windows_subsystem = "windows"]
use std::ffi::c_void;
use std::ptr::null_mut;
type H = *mut c_void;
#[repr(C)]
struct Class {
    style: u32,
    callback: Option<unsafe extern "system" fn(H, u32, usize, isize) -> isize>,
    class_extra: i32, window_extra: i32, instance: H, icon: H, cursor: H,
    background: H, menu: *const u16, name: *const u16,
}
#[repr(C)]
struct Message { window: H, message: u32, wparam: usize, lparam: isize, time: u32, x: i32, y: i32, private: u32 }
#[link(name = "user32")]
extern "system" {
    fn RegisterClassW(class: *const Class) -> u16;
    fn CreateWindowExW(ex: u32, class: *const u16, title: *const u16, style: u32, x: i32, y: i32,
        width: i32, height: i32, parent: H, menu: H, instance: H, param: H) -> H;
    fn DefWindowProcW(window: H, message: u32, wparam: usize, lparam: isize) -> isize;
    fn GetMessageW(message: *mut Message, window: H, min: u32, max: u32) -> i32;
    fn TranslateMessage(message: *const Message) -> i32;
    fn DispatchMessageW(message: *const Message) -> isize;
    fn PostQuitMessage(code: i32);
}
#[link(name = "kernel32")]
extern "system" { fn GetModuleHandleW(name: *const u16) -> H; }

unsafe extern "system" fn callback(window: H, message: u32, wparam: usize, lparam: isize) -> isize {
    if message == 0x10 { return 0; } // Simulate an editor refusing WM_CLOSE until the user saves.
    if message == 2 { PostQuitMessage(0); return 0; }
    DefWindowProcW(window, message, wparam, lparam)
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let arg = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1));
    if let Some(path) = arg("--marker") {
        let payload = arg("--payload").map(String::as_str).unwrap_or("");
        let text = format!("{payload}\n{}", std::env::current_dir().unwrap().display());
        std::fs::write(path, text).unwrap();
        return;
    }
    if args.iter().any(|a| a == "--idle") {
        loop { std::thread::sleep(std::time::Duration::from_secs(1)); }
    }
    if !args.iter().any(|a| a == "--hold-window") {
        std::fs::write(std::env::current_exe().unwrap().with_extension("started"), std::env::current_dir().unwrap().to_string_lossy().as_bytes()).unwrap();
        return;
    }
    unsafe {
        let name: Vec<u16> = "Blitz unsaved test fixture".encode_utf16().chain(Some(0)).collect();
        let instance = GetModuleHandleW(std::ptr::null());
        let class = Class { style: 0, callback: Some(callback), class_extra: 0, window_extra: 0,
            instance, icon: null_mut(), cursor: null_mut(), background: null_mut(), menu: std::ptr::null(), name: name.as_ptr() };
        assert_ne!(RegisterClassW(&class), 0);
        let window = CreateWindowExW(0, name.as_ptr(), name.as_ptr(), 0x10cf0000, 160, 160, 400, 160,
            null_mut(), null_mut(), instance, null_mut());
        assert!(!window.is_null());
        let mut message: Message = std::mem::zeroed();
        while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}
