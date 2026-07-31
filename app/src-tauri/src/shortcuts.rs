use crate::db;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{
    GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState,
};

/// Runtime view of user settings, kept in sync with the database.
pub struct SettingsState(pub Mutex<db::Settings>);

fn toggle_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        #[cfg(target_os = "windows")]
        {
            // 参考 template/OopsInterview 的实现:用 Win32 SetWindowPos 隐蔽显隐,
            // 避免任务栏残留图标,显示时强制 WS_EX_TOOLWINDOW。
            use windows::Win32::Foundation::HWND;
            use windows::Win32::UI::WindowsAndMessaging::{
                GetWindowLongW, IsIconic, IsWindowVisible, SetWindowLongW, SetWindowPos,
                GWL_EXSTYLE, HWND_NOTOPMOST, HWND_TOPMOST, SWP_HIDEWINDOW, SWP_NOACTIVATE,
                SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, WS_EX_TOOLWINDOW,
            };

            let hwnd = match window.hwnd() {
                Ok(h) => h,
                Err(_) => return,
            };

            unsafe {
                let visible = IsWindowVisible(hwnd).as_bool() && !IsIconic(hwnd).as_bool();
                if visible {
                    let _ = SetWindowPos(
                        hwnd,
                        Some(HWND(std::ptr::null_mut())),
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_HIDEWINDOW,
                    );
                } else {
                    let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
                    SetWindowLongW(
                        hwnd,
                        GWL_EXSTYLE,
                        ex_style | (WS_EX_TOOLWINDOW.0 as i32),
                    );
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
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                    );
                    let _ = window.set_focus();
                }
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let visible = window.is_visible().unwrap_or(false);
            if visible {
                let _ = window.hide();
            } else {
                let _ = window.show();
                let _ = window.set_focus();
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

