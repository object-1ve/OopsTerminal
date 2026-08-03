use crate::db;
use crate::shortcuts::SettingsState;
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

/// Holds the runtime tray icon while it is shown.
pub struct TrayState(pub Mutex<Option<TrayIcon>>);

/// 注册托盘菜单的全局菜单事件。只需注册一次,重复创建/销毁托盘时
/// 不会累积监听器(每次 build 时 TrayIconBuilder::on_menu_event 都会 push)。
pub fn register_global_menu_events(app: &AppHandle) {
    app.on_menu_event(|app, event| match event.id.as_ref() {
        "toggle" => crate::shortcuts::toggle_main_window(app),
        "quit" => app.exit(0),
        _ => {}
    });
}

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

/// 显示或隐藏任务栏按钮。
///
/// 采用双保险:
/// 1. 更新 WS_EX_TOOLWINDOW 样式作为持久状态,窗口重建/重启后依然生效;
/// 2. 通过 ITaskbarList::AddTab/DeleteTab 立即添加/移除任务栏按钮,
///    避免运行时仅改样式后 explorer 不刷新的问题。
fn apply_taskbar_style(app: &AppHandle, show: bool) {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
        use windows::Win32::UI::Shell::ITaskbarList;
        use windows::Win32::UI::WindowsAndMessaging::{
            GetWindowLongW, SetWindowLongW, GWL_EXSTYLE, WS_EX_TOOLWINDOW,
        };

        // CLSID_TaskbarList {56FDF344-FD6D-11D0-958A-006097C9A090}
        const CLSID_TASKBAR_LIST: windows::core::GUID =
            windows::core::GUID::from_u128(0x56fdf344_fd6d_11d0_958a_006097c9a090);

        if let Some(window) = app.get_webview_window("main") {
            if let Ok(hwnd) = window.hwnd() {
                unsafe {
                    // 1. 持久化样式
                    let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
                    let updated = if show {
                        ex_style & !(WS_EX_TOOLWINDOW.0 as i32)
                    } else {
                        ex_style | (WS_EX_TOOLWINDOW.0 as i32)
                    };
                    SetWindowLongW(hwnd, GWL_EXSTYLE, updated);

                    // 2. 立即生效:添加/移除任务栏按钮
                    if let Ok(taskbar) =
                        CoCreateInstance::<_, ITaskbarList>(&CLSID_TASKBAR_LIST, None, CLSCTX_ALL)
                    {
                        let _ = taskbar.HrInit();
                        if show {
                            let _ = taskbar.AddTab(hwnd);
                        } else {
                            let _ = taskbar.DeleteTab(hwnd);
                        }
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
        // 仅 drop 本地引用不会移除系统托盘图标:TrayIconBuilder::build() 会把
        // 自身的克隆存入 ResourceTable,必须通过 remove_tray_by_id 取出并释放,
        // 才能触发底层 tray-icon 的 Drop(Shell_NotifyIcon NIM_DELETE)。
        *tray_guard = None;
        let _ = app.remove_tray_by_id("main-tray");
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
