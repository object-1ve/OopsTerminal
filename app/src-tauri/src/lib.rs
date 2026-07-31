mod db;

use db::{init_db, load_window_state, save_window_state, WindowState};
use std::sync::Mutex;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager,
};

pub struct DbState(Mutex<rusqlite::Connection>);

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
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

            app.manage(DbState(Mutex::new(conn)));

            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            // 创建托盘菜单
            let quit_i = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let show_i = MenuItem::with_id(app, "show", "显示窗口", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &quit_i])?;

            // 创建托盘图标
            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => {
                        app.exit(0);
                    }
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(app)?;

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
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
