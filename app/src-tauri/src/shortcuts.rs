use crate::db;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{
    GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState,
};
use tauri::Emitter;

/// 支持的字体文件扩展名(小写)。
const FONT_EXTENSIONS: [&str; 4] = ["ttf", "otf", "woff", "woff2"];

/// FILE_ATTRIBUTE_REPARSE_POINT:路径组件是 junction / 符号链接 / 挂载点。
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;

/// 去掉 Windows 路径前缀 `\\?\` 或 `\??\`(junction 的 read_link 结果常带)。
#[cfg(windows)]
fn strip_windows_prefix(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    let stripped = s
        .strip_prefix(r"\\?\")
        .or_else(|| s.strip_prefix(r"\??\"))
        .unwrap_or(&s);
    PathBuf::from(stripped)
}

/// 判断路径是否带 reparse point(junction/符号链接),不跟随链接。
///
/// 注意不能用 `fs::metadata`(会跟随链接):Windows 对不受信任的装入点
/// (如 Scoop 的 `current` junction)拒绝遍历,跟随会直接返回访问被拒绝。
/// `symlink_metadata` 以 FILE_FLAG_OPEN_REPARSE_POINT 打开,只读取链接自身的
/// 元数据,不遍历目标,因此即使链接不可遍历也能识别并读出指向。
fn is_reparse_point(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        std::fs::symlink_metadata(path)
            .map(|md| md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        std::fs::symlink_metadata(path)
            .map(|md| md.file_type().is_symlink())
            .unwrap_or(false)
    }
}

/// 逐组件解析路径里的 junction / 符号链接,返回真实路径。
///
/// 这解决 Scoop 等工具把 `current` 目录做成 junction、而 Windows 把该
/// junction 视为不受信任装入点拒绝遍历的问题:读取链接指向本身不需要遍历,
/// 解析到真实路径后文件就能被正常打开。
fn resolve_links(path: &Path) -> Result<PathBuf, String> {
    let mut resolved = PathBuf::new();
    for comp in path.components() {
        resolved.push(comp.as_os_str());
        if !is_reparse_point(&resolved) {
            continue;
        }
        let target = std::fs::read_link(&resolved).map_err(|e| {
            format!(
                "无法读取路径 \"{}\" 的链接指向(可能是不受信任的装入点): {e}",
                resolved.display()
            )
        })?;
        #[cfg(windows)]
        let target = strip_windows_prefix(&target);
        let target = if target.is_absolute() {
            target
        } else {
            let base = resolved.parent().unwrap_or_else(|| Path::new("."));
            base.join(target)
        };
        // 继续以链接目标为基准解析后续组件;目标本身可能还含链接,交给循环处理
        resolved = target;
    }
    Ok(resolved)
}

/// 校验并解析终端字体文件路径。
///
/// 返回真实(解析链接后)的绝对路径。校验失败时返回带中文说明的错误,
/// 便于设置界面直接展示给用户。
fn resolve_font_path(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("字体路径为空".into());
    }
    let path = Path::new(trimmed);
    if !path.is_absolute() {
        return Err("字体文件路径必须是绝对路径".into());
    }

    let resolved = resolve_links(path)?;

    let ext = resolved
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    if !matches!(ext.as_deref(), Some(e) if FONT_EXTENSIONS.contains(&e)) {
        return Err("字体文件必须是 ttf/otf/woff/woff2 格式".into());
    }

    let md = std::fs::metadata(&resolved)
        .map_err(|e| format!("无法访问字体文件 \"{}\": {e}", resolved.display()))?;
    if !md.is_file() {
        return Err("字体路径指向的不是文件".into());
    }
    // 再确认真正可读,避免只存在但被权限拒绝
    std::fs::File::open(&resolved)
        .map_err(|e| format!("无法读取字体文件 \"{}\": {e}", resolved.display()))?;

    Ok(resolved)
}

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

/// 保存终端字体文件路径。传 None 或空字符串恢复默认字体。
/// 保存后广播 settings-changed 事件,已打开的终端实时应用新字体。
#[tauri::command]
pub fn set_terminal_font_path(
    app: AppHandle,
    state: tauri::State<'_, SettingsState>,
    path: Option<String>,
) -> Result<db::Settings, String> {
    let raw = path
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    // 保存前先解析 junction/符号链接并校验文件可用,路径不可用就当场报错,
    // 而不是把坏路径存进数据库然后静默超时回退默认字体。
    if let Some(p) = &raw {
        resolve_font_path(p).map_err(|e| format!("字体路径无效: {e}"))?;
    }

    let mut guard = state.0.lock().unwrap();
    if guard.terminal_font_path != raw {
        let db_state = app.state::<crate::DbState>();
        let conn = db_state.0.lock().unwrap();
        db::save_setting(&conn, "terminal_font_path", raw.as_deref())
            .map_err(|e| format!("保存设置失败: {e}"))?;
        guard.terminal_font_path = raw;

        // 通知所有已打开的终端应用新字体
        let _ = app.emit("settings-changed", ());
    }

    Ok(guard.clone())
}

/// 解析并校验终端字体文件路径,返回真实(链接已解析)的绝对路径。
/// 前端在加载字体前调用,解决路径经过不受信任 junction(如 Scoop 的
/// `current`)时直接读取会超时的问题。
#[tauri::command]
pub fn resolve_terminal_font_path(path: String) -> Result<String, String> {
    resolve_font_path(&path).map(|p| p.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
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

    fn test_base_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "oops-term-{name}-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rejects_empty_or_relative_or_bad_extension() {
        assert!(resolve_font_path("").is_err());
        assert!(resolve_font_path("   ").is_err());
        assert!(resolve_font_path("relative/font.ttf").is_err());
        assert!(resolve_font_path(r"C:\Users\me\font.txt").is_err());
    }

    #[test]
    fn rejects_missing_file() {
        let base = test_base_dir("missing");
        let p = base.join("nope.ttf");
        assert!(resolve_font_path(&p.to_string_lossy()).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Windows:路径经过 junction 时应解析到真实目标,而不是卡在不可遍历的链接上。
    /// 用 PowerShell `New-Item -ItemType Junction` 创建 junction(无需管理员
    /// 权限),并把子进程输出重定向到临时文件,避免 `mklink` 的本地化中文输出
    /// 在测试控制台里产生乱码。
    #[cfg(windows)]
    #[test]
    fn resolves_junction_to_real_path() {
        let base = test_base_dir("junction");
        let real_dir = base.join("real/version1");
        std::fs::create_dir_all(&real_dir).unwrap();
        std::fs::write(real_dir.join("dummy.ttf"), b"font").unwrap();

        let link = base.join("current");
        let ps_script = format!(
            "New-Item -ItemType Junction -Path '{}' -Target '{}' | Out-Null",
            link.to_string_lossy(),
            real_dir.to_string_lossy()
        );
        let status = std::process::Command::new("cmd")
            .args(["/C", "powershell", "-NoProfile", "-Command", &ps_script])
            .stdout(std::fs::File::create(base.join("_out.txt")).unwrap())
            .stderr(std::fs::File::create(base.join("_err.txt")).unwrap())
            .status()
            .expect("run powershell to create junction");
        assert!(status.success(), "junction creation failed: {status}");
        // 环境前提:链接本身能被识别为 reparse point
        assert!(is_reparse_point(&link));

        let via_link = link.join("dummy.ttf");
        let resolved = resolve_font_path(&via_link.to_string_lossy()).expect("resolve through junction");
        assert!(
            resolved.starts_with(&real_dir),
            "resolved={} expected prefix={}",
            resolved.display(),
            real_dir.display()
        );
        assert_eq!(resolved.extension().and_then(|e| e.to_str()), Some("ttf"));
        assert!(resolved.is_file(), "resolved file should be readable");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Windows:真实场景回归测试。若本机存在 Scoop 安装的 Maple Mono NF CN,
    /// 则验证保存的 `current` 路径能解析到真实可读文件。
    #[cfg(windows)]
    #[test]
    fn resolves_scoop_current_font_if_present() {
        let scoop_path = Path::new(r"C:\Users\yzz\scoop\apps\Maple-Mono-NF-CN\current\MapleMono-NF-CN-Regular.ttf");
        if !scoop_path.exists() {
            eprintln!("skipping: {scoop_path:?} not present");
            return;
        }
        let resolved = resolve_font_path(&scoop_path.to_string_lossy()).expect("resolve scoop font");
        assert!(resolved.is_file());
        assert_ne!(
            resolved,
            scoop_path,
            "path should have been resolved away from the `current` junction"
        );
        eprintln!("resolved scoop font -> {resolved:?}");
    }
}

