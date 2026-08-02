use crate::db;
use crate::shortcuts::SettingsState;
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

/// Holds the runtime tray icon while it is shown. When set to `None` (or dropped),
/// the tray icon is removed from the system tray.
pub struct TrayState(pub Mutex<Option<TrayIcon>>);

fn create_tray(app: &AppHandle) -> tauri::Result<TrayIcon> {
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| tauri::Error::AssetNotFound("window icon".into()))?;

    let toggle = MenuItem::with_id(app, "toggle", "显示/隐藏窗口", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&toggle, &quit])?;

    let tray = TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .tooltip("OopsTerminal")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "toggle" => crate::shortcuts::toggle_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                crate::shortcuts::toggle_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(tray)
}

/// Show or hide the taskbar button by toggling the WS_EX_TOOLWINDOW style.
fn apply_taskbar_style(app: &AppHandle, show: bool) {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::WindowsAndMessaging::{
            GetWindowLongW, IsWindowVisible, SetWindowLongW, SetWindowPos, ShowWindow,
            GWL_EXSTYLE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW,
            SW_HIDE, SW_SHOW, WS_EX_TOOLWINDOW,
        };

        if let Some(window) = app.get_webview_window("main") {
            if let Ok(hwnd) = window.hwnd() {
                unsafe {
                    let was_visible = IsWindowVisible(hwnd).as_bool();
                    let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
                    let updated = if show {
                        ex_style & !(WS_EX_TOOLWINDOW.0 as i32)
                    } else {
                        ex_style | (WS_EX_TOOLWINDOW.0 as i32)
                    };
                    SetWindowLongW(hwnd, GWL_EXSTYLE, updated);

                    if was_visible {
                        // 重新显示窗口,强制任务栏按新样式重新评估,立即隐藏/显示任务栏按钮。
                        let _ = ShowWindow(hwnd, SW_HIDE);
                        let _ = SetWindowPos(
                            hwnd,
                            None,
                            0,
                            0,
                            0,
                            0,
                            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                        );
                        let _ = ShowWindow(hwnd, SW_SHOW);
                    }
                }
            }
        }
    }
}

/// Apply tray + taskbar visibility settings at startup or when they change.
pub fn apply_ui_settings(app: &AppHandle, settings: &db::Settings) {
    let tray_state = app.state::<TrayState>();
    let mut tray_guard = tray_state.0.lock().unwrap();
    if settings.show_tray_icon && tray_guard.is_none() {
        match create_tray(app) {
            Ok(tray) => *tray_guard = Some(tray),
            Err(e) => log::warn!("failed to create tray icon: {e}"),
        }
    } else if !settings.show_tray_icon {
        // Dropping the TrayIcon removes it from the system tray.
        *tray_guard = None;
    }
    drop(tray_guard);

    apply_taskbar_style(app, settings.show_taskbar_icon);
}

/// 保存托盘图标与任务栏图标的显示设置,并立即生效。
#[tauri::command]
pub fn set_ui_settings(
    app: AppHandle,
    state: tauri::State<'_, SettingsState>,
    show_tray_icon: bool,
    show_taskbar_icon: bool,
) -> Result<db::Settings, String> {
    let mut guard = state.0.lock().unwrap();
    if guard.show_tray_icon != show_tray_icon || guard.show_taskbar_icon != show_taskbar_icon {
        guard.show_tray_icon = show_tray_icon;
        guard.show_taskbar_icon = show_taskbar_icon;

        let db_state = app.state::<crate::DbState>();
        let conn = db_state.0.lock().unwrap();
        db::save_setting(&conn, "show_tray_icon", Some(&show_tray_icon.to_string()))
            .map_err(|e| format!("保存设置失败: {e}"))?;
        db::save_setting(&conn, "show_taskbar_icon", Some(&show_taskbar_icon.to_string()))
            .map_err(|e| format!("保存设置失败: {e}"))?;

        apply_ui_settings(&app, &guard);
    }

    Ok(guard.clone())
}
