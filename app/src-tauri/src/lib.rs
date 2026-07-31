mod db;
mod shortcuts;
mod terminal;

use db::{init_db, load_settings, load_window_state, save_window_state, WindowState};
use shortcuts::SettingsState;
use std::sync::Mutex;
use tauri::Manager;

pub struct DbState(Mutex<rusqlite::Connection>);

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|app| {
            // 初始化数据库路径
            let app_data_dir = app
                .path()
                .app_data_dir()
                .expect("failed to get app data dir");
            if !app_data_dir.exists() {
                std::fs::create_dir_all(&app_data_dir).expect("failed to create app data dir");
            }

            let conn = init_db(app_data_dir).expect("failed to initialize database");

            // 尝试加载上次保存的窗口状态
            if let Ok(Some(state)) = load_window_state(&conn) {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.set_size(tauri::Size::Physical(tauri::PhysicalSize {
                        width: state.width as u32,
                        height: state.height as u32,
                    }));
                    let _ =
                        window.set_position(tauri::Position::Physical(tauri::PhysicalPosition {
                            x: state.x,
                            y: state.y,
                        }));
                }
            }

            // 加载设置并注册全局快捷键
            let settings = load_settings(&conn).unwrap_or(db::Settings {
                toggle_window_shortcut: None,
                quit_shortcut: None,
            });

            app.manage(DbState(Mutex::new(conn)));

            let app_handle = app.handle();
            shortcuts::init_shortcuts(app_handle, &settings);
            app.manage(SettingsState(Mutex::new(settings)));
            app.manage(terminal::TerminalManager::default());

            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            // 隐藏任务栏图标:强制 WS_EX_TOOLWINDOW 扩展样式(与模板 OopsInterview 一致)
            #[cfg(target_os = "windows")]
            {
                use windows::Win32::UI::WindowsAndMessaging::{
                    GetWindowLongW, SetWindowLongW, GWL_EXSTYLE, WS_EX_TOOLWINDOW,
                };

                if let Some(window) = app.get_webview_window("main") {
                    if let Ok(hwnd) = window.hwnd() {
                        unsafe {
                            let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
                            SetWindowLongW(hwnd, GWL_EXSTYLE, ex_style | (WS_EX_TOOLWINDOW.0 as i32));
                        }
                    }
                }
            }

            Ok(())
        })
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::CloseRequested { api, .. } => {
                window.hide().unwrap();
                api.prevent_close();
            }
            tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) => {
                let app = window.app_handle();
                if let Some(db_state) = app.try_state::<DbState>() {
                    let conn = db_state.0.lock().unwrap();
                    let size = window.inner_size().unwrap();
                    let pos = window.outer_position().unwrap();

                    let state = WindowState {
                        width: size.width as f64,
                        height: size.height as f64,
                        x: pos.x,
                        y: pos.y,
                    };

                    let _ = save_window_state(&conn, &state);
                }
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            shortcuts::get_settings,
            shortcuts::set_shortcut,
            terminal::create_terminal,
            terminal::write_terminal,
            terminal::resize_terminal,
            terminal::kill_terminal,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
