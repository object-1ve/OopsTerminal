mod db;
mod shortcuts;
mod terminal;
mod ui;

use db::{init_db, load_settings};
use shortcuts::SettingsState;
use std::sync::Mutex;
use tauri::Manager;

pub struct DbState(Mutex<rusqlite::Connection>);

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // 已有一个实例在运行时,新实例直接退出,并唤起已有实例的主窗口
            shortcuts::show_main_window(app);
        }))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
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

            // 每次启动窗口居中显示,不记忆上次的位置与大小
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.center();
            }

            // 加载设置并注册全局快捷键
            let settings = load_settings(&conn).unwrap_or(db::Settings {
                toggle_window_shortcut: None,
                quit_shortcut: None,
                default_path: None,
                show_tray_icon: true,
                show_taskbar_icon: false,
                terminal_font_path: None,
            });

            app.manage(DbState(Mutex::new(conn)));
            app.manage(ui::TrayState(Mutex::new(None)));

            let app_handle = app.handle();
            shortcuts::init_shortcuts(app_handle, &settings);
            app.manage(SettingsState(Mutex::new(settings)));
            app.manage(terminal::TerminalManager::default());

            // 托盘菜单事件全局注册一次,避免重复创建托盘时监听器累积
            ui::register_global_menu_events(&app_handle);

            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            // 根据设置应用托盘图标与任务栏图标的显示状态
            let settings_state = app.state::<SettingsState>();
            let settings_ref = settings_state.0.lock().unwrap().clone();
            ui::apply_ui_settings(&app_handle, &settings_ref);

            Ok(())
        })
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::CloseRequested { api, .. } => {
                window.hide().unwrap();
                api.prevent_close();
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            shortcuts::get_settings,
            shortcuts::set_shortcut,
            shortcuts::set_default_path,
            shortcuts::set_terminal_font_path,
            shortcuts::resolve_terminal_font_path,
            ui::set_ui_settings,
            terminal::create_terminal,
            terminal::write_terminal,
            terminal::resize_terminal,
            terminal::kill_terminal,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
