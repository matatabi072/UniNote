//! UniNote Desktop
//! SimpleTask Desktop + SimpleNote Desktop を統合した軽量・ポータブルアプリ（egui/eframe・単一exe）。
//! タスク管理とメモをタブで切り替え。tasks.json / notes.json は個別アプリと互換。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod model;
mod settings;
mod storage;

use eframe::egui;

/// 二重起動防止。既に起動中なら既存ウィンドウを前面化し true を返す。
#[cfg(windows)]
fn already_running() -> bool {
    use std::ffi::{c_void, OsStr};
    use std::os::windows::ffi::OsStrExt;

    extern "system" {
        fn CreateMutexW(attr: *const c_void, owner: i32, name: *const u16) -> *mut c_void;
        fn GetLastError() -> u32;
        fn FindWindowW(class_name: *const u16, window_name: *const u16) -> *mut c_void;
        fn ShowWindow(hwnd: *mut c_void, cmd: i32) -> i32;
        fn SetForegroundWindow(hwnd: *mut c_void) -> i32;
    }
    const ERROR_ALREADY_EXISTS: u32 = 183;
    const SW_RESTORE: i32 = 9;

    let to_wide =
        |s: &str| -> Vec<u16> { OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect() };

    unsafe {
        let name = to_wide("UniNote_SingleInstance_Mutex");
        let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if handle.is_null() {
            return false;
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let title = to_wide("UniNote");
            let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
            if !hwnd.is_null() {
                ShowWindow(hwnd, SW_RESTORE);
                SetForegroundWindow(hwnd);
            }
            return true;
        }
        false
    }
}

#[cfg(not(windows))]
fn already_running() -> bool {
    false
}

fn main() -> eframe::Result<()> {
    if already_running() {
        return Ok(());
    }

    let saved = storage::load_settings();
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size(saved.window_size.unwrap_or([560.0, 600.0]))
        .with_min_inner_size([280.0, 200.0])
        .with_title("UniNote");
    if let Some(pos) = saved.window_pos {
        viewport = viewport.with_position(pos);
    }
    if saved.always_on_top {
        viewport = viewport.with_window_level(egui::WindowLevel::AlwaysOnTop);
    }

    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "UniNote",
        native_options,
        Box::new(|cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(app::App::new(cc)))
        }),
    )
}
