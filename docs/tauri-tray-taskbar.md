# Tauri 2 托盘图标与任务栏图标控制技术方案

> 适用:Tauri 2.x(Rust 后端)+ Windows 10/11。
> 来源:OopsTerminal 实战修复(见 `app/logs.md` 第 4 条),已通过运行时真机验证。

## 目录

1. [核心结论](#核心结论)
2. [托盘图标(TrayIcon)](#1-托盘图标trayicon)
3. [任务栏图标控制](#2-任务栏图标控制)
4. [菜单事件监听器](#3-菜单事件监听器)
5. [Windows 11 托盘溢出区说明](#4-windows-11-托盘溢出区说明)
6. [验证与调试方法](#5-验证与调试方法)
7. [踩坑清单](#6-踩坑清单)

## 核心结论

| 功能 | 正确做法 | 常见错误 |
|------|----------|----------|
| 创建托盘图标 | `TrayIconBuilder::build()` | 无 |
| 移除托盘图标 | `app.remove_tray_by_id(id)` 释放 ResourceTable 引用 | 只 drop 局部变量 → 图标残留 |
| 运行时切换任务栏按钮 | `ITaskbarList::AddTab/DeleteTab` | 只改 `WS_EX_TOOLWINDOW` 样式 → 不刷新 |
| 托盘菜单事件 | `AppHandle::on_menu_event` 全局注册一次 | `TrayIconBuilder::on_menu_event` 每次 build 都注册 → 监听器累积 |

---

## 1. 托盘图标(TrayIcon)

### 1.1 创建

```rust
fn create_tray(app: &AppHandle) -> tauri::Result<TrayIcon> {
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| tauri::Error::AssetNotFound("window icon".into()))?;

    let menu = Menu::with_items(app, &[&toggle_item, &quit_item])?;

    TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .tooltip("AppName")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            // 左键单击切换窗口显隐
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                crate::shortcuts::toggle_main_window(tray.app_handle());
            }
        })
        .build(app)
}
```

### 1.2 移除(最容易踩的坑)

**背景**:tauri 的 `TrayIcon` 是引用计数类型(`Arc` 语义)。`TrayIconBuilder::build()` 内部会调用 `register()`,把自己的克隆存入全局 `ResourceTable`(`AppManager.tray.icons` + `resources_table`)。

**后果**:你持有的局部变量 drop 后,ResourceTable 里仍有一份强引用,底层 `tray-icon` crate 的 `Drop`(真正执行 `Shell_NotifyIcon(NIM_DELETE)` 的地方)永远不会触发,托盘图标残留在系统托盘。

**正确做法**:

```rust
// 需要持有状态以便判断当前是否已创建
pub struct TrayState(pub Mutex<Option<TrayIcon>>);

pub fn apply_ui_settings(app: &AppHandle, settings: &db::Settings) {
    let tray_state = app.state::<TrayState>();
    let mut tray_guard = tray_state.0.lock().unwrap();

    if settings.show_tray_icon && tray_guard.is_none() {
        match create_tray(app) {
            Ok(tray) => *tray_guard = Some(tray),
            Err(e) => log::warn!("failed to create tray icon: {e}"),
        }
    } else if !settings.show_tray_icon {
        // 关键:必须同时释放 ResourceTable 中的引用
        // 只执行 *tray_guard = None 无法移除系统托盘图标!
        *tray_guard = None;
        let _ = app.remove_tray_by_id("main-tray");
    }
    drop(tray_guard);
}
```

`remove_tray_by_id` 内部执行:`resources_table().take(rid)` 取出 Arc + `icon.close()` 从 icons 列表移除,局部 Arc 释放后触发底层 Drop,托盘图标才真正消失。

> 依赖要求:`tauri = { version = "2", features = ["tray-icon"] }`

---

## 2. 任务栏图标控制

### 2.1 为什么只改样式不行

经典做法是切换 `WS_EX_TOOLWINDOW` 扩展样式让窗口不显示在任务栏。经验证:

- **启动时**设置样式:有效(窗口创建初期 explorer 按样式评估)。
- **运行时**切换样式 + `SetWindowPos` 刷新:不可靠,explorer 经常不重新评估,任务栏按钮残留。

### 2.2 双保险方案(推荐)

```rust
// Cargo.toml 需要
// windows = { version = "0.61", features = [
//     "Win32_Foundation",
//     "Win32_UI_WindowsAndMessaging",
//     "Win32_UI_Shell",
//     "Win32_System_Com",
// ] }

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
                    // 1. 持久化样式:窗口重建/重启后依然生效
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
```

**原理**:`ITaskbarList::AddTab/DeleteTab` 直接通知任务栏添加/移除指定窗口按钮,立即生效且无副作用;`WS_EX_TOOLWINDOW` 作为持久状态保证后续窗口重建时行为一致。

---

## 3. 菜单事件监听器

`TrayIconBuilder::on_menu_event` 每次 `build()` 都会向 `AppManager.menu.global_event_listeners`(Vec)push 一个闭包。如果程序支持运行时开关托盘(如设置里关闭再打开),每次创建都会累积一个监听器,最终菜单点击一次触发多次。

**正确做法**:在 `setup` 中注册一次全局菜单事件,创建托盘时不再传 `on_menu_event`:

```rust
// lib.rs setup 中调用一次
pub fn register_global_menu_events(app: &AppHandle) {
    app.on_menu_event(|app, event| match event.id.as_ref() {
        "toggle" => crate::shortcuts::toggle_main_window(app),
        "quit" => app.exit(0),
        _ => {}
    });
}
```

`AppHandle::on_menu_event` 注册的是全局菜单事件(任何菜单触发都会回调,按 `event.id` 分发),与托盘是否创建无关,天然只注册一次。

> 注意:`on_tray_icon_event` 是按托盘 id 存 HashMap 的,`remove_tray_by_id` 时随之清理,不存在累积问题,可以继续在 builder 里注册。

---

## 4. Windows 11 托盘溢出区说明

**重要**:Windows 11 上,新注册的托盘图标默认折叠在托盘溢出区(任务栏角落的小箭头),不会出现在主托盘!这是系统行为,不是代码问题。

- 图标首次出现在溢出区后,用户可手动拖到主托盘,系统记住位置。
- 排查"托盘图标没生效"时,务必先展开溢出区再确认。

**主托盘 vs 溢出区验证**:

```text
主托盘(Shell_TrayWnd 下):显示已固定的图标
溢出区(TopLevelWindowForOverflowXamlIsland):显示新注册/未固定的图标
```

---

## 5. 验证与调试方法

### 5.1 UIA 枚举托盘按钮(自动化验证)

```powershell
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

# 找任务栏
$cond = New-Object System.Windows.Automation.PropertyCondition(
  [System.Windows.Automation.AutomationElement]::ClassNameProperty, 'Shell_TrayWnd')
$tray = [System.Windows.Automation.AutomationElement]::RootElement.FindFirst(
  [System.Windows.Automation.TreeScope]::Children, $cond)

# 枚举所有按钮
$btnCond = New-Object System.Windows.Automation.PropertyCondition(
  [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
  [System.Windows.Automation.ControlType]::Button)
$buttons = $tray.FindAll([System.Windows.Automation.TreeScope]::Descendants, $btnCond)
foreach ($b in $buttons) {
  Write-Output ('BTN [' + $b.Current.Name + '] class=[' + $b.Current.ClassName + ']')
}
```

- 任务栏应用按钮:`class = Taskbar.TaskListButtonAutomationPeer`
- 托盘图标按钮:`class = SystemTray.NormalButton`
- 托盘图标 name = tooltip;tooltip 为空时为 `<EMPTY>`

### 5.2 展开溢出区枚举(验证被折叠的图标)

```powershell
# 1. 鼠标点击"显示隐藏的图标"按钮(第一个 SystemTray.NormalButton)
# 2. 枚举 TopLevelWindowForOverflowXamlIsland 下的元素
$cond = New-Object System.Windows.Automation.PropertyCondition(
  [System.Windows.Automation.AutomationElement]::ClassNameProperty,
  'TopLevelWindowForOverflowXamlIsland')
$overflow = [System.Windows.Automation.AutomationElement]::RootElement.FindFirst(
  [System.Windows.Automation.TreeScope]::Children, $cond)
# 其下 SystemTray.NormalButton 的 Name 即为各图标 tooltip
```

### 5.3 Shell_NotifyIconGetRect 验证图标真实注册

`NIM_ADD` 返回成功不代表图标一定可见(如 explorer 异常)。用 `Shell_NotifyIconGetRect` 查询图标实际屏幕位置,`S_OK` 且矩形有效即注册成功:

```rust
let mut ident: NOTIFYICONIDENTIFIER = std::mem::zeroed();
ident.cb_size = std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32;
ident.hwnd = hwnd;
ident.u_id = internal_id;
let mut rect: RECT = std::mem::zeroed();
let hr = Shell_NotifyIconGetRect(&ident, &mut rect); // S_OK = 0x0
```

### 5.4 CDP 远程调试(调用真实 IPC 验证前端→后端链路)

给 WebView 加调试参数后,可用 Chrome DevTools Protocol 直接在页面里执行 `window.__TAURI_INTERNALS__.invoke(...)`,无需操作 UI 即可验证命令链路:

```json
// tauri.conf.json(仅调试用,验证后移除)
"additionalBrowserArgs": "--remote-debugging-port=9222"
```

```powershell
# 获取页面 WebSocket URL
$pages = Invoke-RestMethod -Uri 'http://localhost:9222/json'
# 连接 webSocketDebuggerUrl,发送 Runtime.evaluate:
# window.__TAURI_INTERNALS__.invoke('set_ui_settings',
#   { showTrayIcon: false, showTaskbarIcon: false }).then(r => JSON.stringify(r))
```

验证后务必移除 `additionalBrowserArgs`。

---

## 6. 踩坑清单

| # | 现象 | 根因 | 解法 |
|---|------|------|------|
| 1 | 设置关闭托盘后图标仍残留 | 只 drop 局部引用,ResourceTable 仍持有 | `app.remove_tray_by_id(id)` |
| 2 | 运行时切换任务栏不生效 | 只改样式 explorer 不刷新 | `ITaskbarList::AddTab/DeleteTab` |
| 3 | 反复开关托盘后菜单点一次触发多次 | builder 注册的监听器累积 | `AppHandle::on_menu_event` 注册一次 |
| 4 | 托盘图标"不见了" | Windows 11 默认折叠进溢出区 | 展开溢出区确认,非代码问题 |
| 5 | `cbSize=0` 导致 NIM_ADD 失败 | 误判 | `cbSize` 为 0 实测仍可成功,无需怀疑 |

## 相关代码

- 完整实现:`app/src-tauri/src/ui.rs`
- 依赖配置:`app/src-tauri/Cargo.toml`(`tray-icon` feature、`Win32_UI_Shell`、`Win32_System_Com`)
- 修复记录:`app/logs.md` 第 4 条
