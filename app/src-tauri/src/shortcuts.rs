use crate::db;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{
    GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState,
};
use tauri::Emitter;

/// Runtime view of user settings, kept in sync with the database.
pub struct SettingsState(pub Mutex<db::Settings>);

/// 显示主窗口并聚焦。单实例回调与快捷键的显示分支共用。
pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        #[cfg(target_os = "windows")]
        {
            // 参考 template/OopsInterview 的实现:用 Win32 SetWindowPos 显示窗口,
            // 避免任务栏残留图标,仅在隐藏任务栏图标时强制 WS_EX_TOOLWINDOW。
            use windows::Win32::UI::WindowsAndMessaging::{
                GetWindowLongW, SetWindowLongW, SetWindowPos, GWL_EXSTYLE, HWND_NOTOPMOST,
                HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WS_EX_TOOLWINDOW,
            };

            // 关键:必须先通过 tauri/tao 更新 VISIBLE 状态,不能用 SetWindowPos
            // 直接显示。否则 tao 的 window_flags 里 VISIBLE 仍为 false,之后任何
            // flags 操作(如最大化/还原)都会触发 apply_diff 里的
            // `!new.contains(VISIBLE) => ShowWindow(SW_HIDE)`,把窗口重新隐藏
            // (用户看到的表现就是"最小化")。
            let _ = window.unminimize();
            let _ = window.show();

            let hwnd = match window.hwnd() {
                Ok(h) => h,
                Err(_) => return,
            };

            unsafe {
                // 仅在隐藏任务栏图标时强制 WS_EX_TOOLWINDOW,否则让窗口出现在任务栏
                let show_taskbar = app
                    .state::<SettingsState>()
                    .0
                    .lock()
                    .unwrap()
                    .show_taskbar_icon;
                let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
                let updated = if show_taskbar {
                    ex_style & !(WS_EX_TOOLWINDOW.0 as i32)
                } else {
                    ex_style | (WS_EX_TOOLWINDOW.0 as i32)
                };
                SetWindowLongW(hwnd, GWL_EXSTYLE, updated);
                // 尊重用户的置顶设置,而不是无条件置顶
                let insert_after = if window.is_always_on_top().unwrap_or(false) {
                    Some(HWND_TOPMOST)
                } else {
                    Some(HWND_NOTOPMOST)
                };
                let _ = SetWindowPos(
                    hwnd,
                    insert_after,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
                let _ = window.set_focus();
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = window.show();
            let _ = window.set_focus();
        }
    }
}

pub fn toggle_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        #[cfg(target_os = "windows")]
        {
            // 统一走 tauri 的 hide/show,保证 tao 的 VISIBLE flag 与真实窗口
            // 状态同步,避免后续最大化/还原操作触发 SW_HIDE。
            let visible = window.is_visible().unwrap_or(false)
                && !window.is_minimized().unwrap_or(false);
            if visible {
                let _ = window.hide();
            } else {
                show_main_window(app);
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let visible = window.is_visible().unwrap_or(false);
            if visible {
                let _ = window.hide();
            } else {
                show_main_window(app);
            }
        }
    }
}

fn shortcut_handler(
    kind: &'static str,
) -> impl Fn(&AppHandle, &Shortcut, ShortcutEvent) + Send + Sync + 'static {
    move |app, _shortcut, event| {
        if event.state() != ShortcutState::Pressed {
            return;
        }
        match kind {
            "toggle" => toggle_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        }
    }
}

fn register_shortcut(app: &AppHandle, kind: &'static str, accel: &str) -> Result<(), String> {
    let shortcut: Shortcut = accel
        .parse()
        .map_err(|e| format!("无效的快捷键格式: {e}"))?;
    app.global_shortcut()
        .on_shortcut(shortcut, shortcut_handler(kind))
        .map_err(|e| format!("快捷键注册失败(可能已被其他程序占用): {e}"))
}

fn unregister_shortcut(app: &AppHandle, accel: &str) {
    let _ = app.global_shortcut().unregister(accel);
}

/// Load persisted settings, register every configured shortcut.
pub fn init_shortcuts(app: &AppHandle, settings: &db::Settings) {
    if let Some(accel) = &settings.toggle_window_shortcut {
        if let Err(e) = register_shortcut(app, "toggle", accel) {
            log::warn!("toggle shortcut: {e}");
        }
    }
    if let Some(accel) = &settings.quit_shortcut {
        if let Err(e) = register_shortcut(app, "quit", accel) {
            log::warn!("quit shortcut: {e}");
        }
    }
}

#[tauri::command]
pub fn get_settings(state: tauri::State<'_, SettingsState>) -> db::Settings {
    state.0.lock().unwrap().clone()
}

#[tauri::command]
pub fn set_shortcut(
    app: AppHandle,
    state: tauri::State<'_, SettingsState>,
    kind: String,
    accel: Option<String>,
) -> Result<db::Settings, String> {
    let db_key = match kind.as_str() {
        "toggle" => "toggle_window_shortcut",
        "quit" => "quit_shortcut",
        _ => return Err("未知的设置项".into()),
    };
    let action_kind: &'static str = match kind.as_str() {
        "toggle" => "toggle",
        _ => "quit",
    };

    let accel = accel.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let mut guard = state.0.lock().unwrap();
    let old = match db_key {
        "toggle_window_shortcut" => guard.toggle_window_shortcut.clone(),
        _ => guard.quit_shortcut.clone(),
    };

    if old != accel {
        // Register the new shortcut first so a conflict leaves the old one intact.
        if let Some(new) = &accel {
            register_shortcut(&app, action_kind, new)?;
        }
        if let Some(old_accel) = &old {
            unregister_shortcut(&app, old_accel);
        }

        let db_state = app.state::<crate::DbState>();
        let conn = db_state.0.lock().unwrap();
        db::save_setting(&conn, db_key, accel.as_deref())
            .map_err(|e| format!("保存设置失败: {e}"))?;

        match db_key {
            "toggle_window_shortcut" => guard.toggle_window_shortcut = accel,
            _ => guard.quit_shortcut = accel,
        }
    }

    Ok(guard.clone())
}

/// 保存终端默认启动路径。传 None 或空字符串表示使用用户主目录。
#[tauri::command]
pub fn set_default_path(
    app: AppHandle,
    state: tauri::State<'_, SettingsState>,
    path: Option<String>,
) -> Result<db::Settings, String> {
    let path = path
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let mut guard = state.0.lock().unwrap();
    if guard.default_path != path {
        let db_state = app.state::<crate::DbState>();
        let conn = db_state.0.lock().unwrap();
        db::save_setting(&conn, "default_path", path.as_deref())
            .map_err(|e| format!("保存设置失败: {e}"))?;
        guard.default_path = path;
    }

    Ok(guard.clone())
}

/// 保存终端字体 (CSS font-family)。传 None 或空字符串恢复默认字体。
/// 保存后广播 settings-changed 事件,已打开的终端实时应用新字体。
#[tauri::command]
pub fn set_terminal_font(
    app: AppHandle,
    state: tauri::State<'_, SettingsState>,
    font: Option<String>,
) -> Result<db::Settings, String> {
    let font = font
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let mut guard = state.0.lock().unwrap();
    if guard.terminal_font != font {
        let db_state = app.state::<crate::DbState>();
        let conn = db_state.0.lock().unwrap();
        db::save_setting(&conn, "terminal_font", font.as_deref())
            .map_err(|e| format!("保存设置失败: {e}"))?;
        guard.terminal_font = font;

        // 通知所有已打开的终端应用新字体
        let _ = app.emit("settings-changed", ());
    }

    Ok(guard.clone())
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;
    use tauri_plugin_global_shortcut::Shortcut;

    #[test]
    fn parses_frontend_accelerator_format() {
        for accel in ["Ctrl+Shift+K", "Alt+1", "Super+F5", "Shift+F1", "Ctrl+Alt+Delete"] {
            assert!(
                Shortcut::from_str(accel).is_ok(),
                "{accel} should parse"
            );
        }
    }

    #[test]
    fn rejects_modifier_only_or_empty() {
        assert!(Shortcut::from_str("").is_err());
        assert!(Shortcut::from_str("Ctrl+Shift").is_err());
        assert!(Shortcut::from_str("Ctrl").is_err());
    }
}

