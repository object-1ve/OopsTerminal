# Tauri 2 自定义标题栏窗口状态同步技术方案

> 适用:Tauri 2.x(Rust 后端)+ Windows,使用自定义标题栏(`decorations: false`)且窗口支持隐藏到托盘/后台的桌面应用。
> 来源:OopsTerminal 实战修复(见 `app/logs.md`),已通过真机复现验证。

## 目录

1. [问题现象](#1-问题现象)
2. [根因分析](#2-根因分析)
3. [修复方案](#3-修复方案)
4. [核心原理:tao 窗口状态机](#4-核心原理tao-窗口状态机)
5. [易错点清单](#5-易错点清单)
6. [相关代码](#6-相关代码)

## 1. 问题现象

用户操作序列:

1. 点击自定义标题栏**关闭按钮** → 窗口隐藏到托盘(符合预期,`CloseRequested` 拦截并 `hide()`)
2. 从托盘唤出窗口
3. 点击**最大化按钮** → **窗口最小化**(而非最大化)
4. 点击**还原按钮** → 同样触发最小化

仅"刚启动 → 直接最大化 → 还原"时正常。一旦经历"关闭(隐藏) → 唤出"流程后,最大化/还原就异常。

## 2. 根因分析

### 2.1 tao 的窗口状态机

tauri 在 Windows 底层使用 `tao` 管理窗口。tao 维护一个 `WindowFlags` 位掩码(`window_state.rs`),其中:

- `VISIBLE` — tao 认为窗口是否可见
- `MAXIMIZED` / `MINIMIZED` — 窗口最大化 / 最小化状态

所有窗口操作(show/hide/maximize/minimize)都会走 `WindowState::set_window_flags` → `apply_diff`,**通过修改 flags 再统一应用**:

```rust
// tao-0.35.3 window_state.rs apply_diff 关键代码
if diff.contains(WindowFlags::MAXIMIZED) || new.contains(WindowFlags::MAXIMIZED) {
    ShowWindow(window, if new.contains(MAXIMIZED) { SW_MAXIMIZE } else { SW_RESTORE });
}

// ...其他 flags 处理...

// 致命的一行:只要 tao 认为窗口不可见,就隐藏
if !new.contains(WindowFlags::VISIBLE) {
    ShowWindow(window, SW_HIDE);
}
```

### 2.2 绕过状态机的隐藏/显示

原实现 `show_main_window` 用 Win32 API 直接显示窗口:

```rust
// 错误做法:绕过 tao 状态机
SetWindowPos(hwnd, ..., SWP_SHOWWINDOW);  // 窗口真实可见了
```

窗口虽然真实显示,但 tao 的 `WindowFlags::VISIBLE` 仍是 `false`(因为隐藏时 `CloseRequested` 走的是 `window.hide()`,tao 把 flag 置 false)。

### 2.3 触发链条

```
关闭按钮 → window.hide()           → tao VISIBLE=false
托盘唤出 → SetWindowPos(SW_SHOWWINDOW) → 真实可见,但 VISIBLE 仍 false
点最大化 → toggleMaximize()
        → set_maximized(true)
        → apply_diff:
            diff 含 MAXIMIZED → ShowWindow(SW_MAXIMIZE)  ← 窗口先最大化
            new 不含 VISIBLE  → ShowWindow(SW_HIDE)     ← 立即又被隐藏!
结果:窗口"最小化"(实际是被隐藏,任务栏按钮消失)
```

还原同理:`unmaximize` 触发 `apply_diff`,同样走到 `SW_HIDE`。

## 3. 修复方案

**核心原则:所有窗口显示/隐藏操作必须走 tauri API(`window.show()/hide()`),禁止用 Win32 `SetWindowPos` 绕过 tao 状态机。**

```rust
/// 显示主窗口并聚焦
pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        #[cfg(target_os = "windows")]
        {
            use windows::Win32::UI::WindowsAndMessaging::{
                GetWindowLongW, SetWindowLongW, SetWindowPos, GWL_EXSTYLE,
                HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
                WS_EX_TOOLWINDOW,
            };

            // 关键:必须先通过 tauri/tao 更新 VISIBLE 状态
            // 不能用 SetWindowPos 直接显示,否则 tao 的 window_flags 里
            // VISIBLE 仍为 false,之后任何 flags 操作(最大化/还原)都会
            // 触发 apply_diff 的 !VISIBLE => SW_HIDE,窗口被重新隐藏
            // (表现为"最小化")。
            let _ = window.unminimize();
            let _ = window.show();

            let hwnd = match window.hwnd() {
                Ok(h) => h,
                Err(_) => return,
            };

            unsafe {
                // 仅调整扩展样式(任务栏显示策略)与 Z 序,不再做显示操作
                let show_taskbar = app
                    .state::<SettingsState>()
                    .0.lock().unwrap().show_taskbar_icon;
                let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
                let updated = if show_taskbar {
                    ex_style & !(WS_EX_TOOLWINDOW.0 as i32)
                } else {
                    ex_style | (WS_EX_TOOLWINDOW.0 as i32)
                };
                SetWindowLongW(hwnd, GWL_EXSTYLE, updated);

                let insert_after = if window.is_always_on_top().unwrap_or(false) {
                    Some(HWND_TOPMOST)
                } else {
                    Some(HWND_NOTOPMOST)
                };
                let _ = SetWindowPos(
                    hwnd, insert_after, 0, 0, 0, 0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE, // 无 SWP_SHOWWINDOW
                );
                let _ = window.set_focus();
            }
        }
        // 非 Windows 平台直接 show
        #[cfg(not(target_os = "windows"))]
        {
            let _ = window.show();
            let _ = window.set_focus();
        }
    }
}
```

隐藏分支同样改为走 tauri API:

```rust
pub fn toggle_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let visible = window.is_visible().unwrap_or(false)
            && !window.is_minimized().unwrap_or(false);
        if visible {
            let _ = window.hide();       // 走 tao,同步 VISIBLE flag
        } else {
            show_main_window(app);       // 走 tao show,同步 VISIBLE flag
        }
    }
}
```

## 4. 核心原理:tao 窗口状态机

理解这个 bug 需要掌握 tao 的窗口操作模型:

```
用户操作 → tauri API → 修改 WindowFlags → apply_diff → 统一 ShowWindow/SetWindowPos
              ↑
          (唯一正确入口)
```

**tauri API 列表与对应 flags:**

| tauri API | tao 行为 |
|-----------|----------|
| `window.show()` | 设 `VISIBLE=true`, `ShowWindow(SW_SHOW)` |
| `window.hide()` | 设 `VISIBLE=false`, `ShowWindow(SW_HIDE)` |
| `window.maximize()` | 设 `MAXIMIZED=true`, `ShowWindow(SW_MAXIMIZE)` |
| `window.unmaximize()` | 设 `MAXIMIZED=false`, `ShowWindow(SW_RESTORE)` |
| `window.minimize()` | 设 `MINIMIZED=true`, `ShowWindow(SW_MINIMIZE)` |
| `window.unminimize()` | 设 `MINIMIZED=false`, `ShowWindow(SW_RESTORE)` |

**为什么不能混用 Win32 API:**

`apply_diff` 每次执行都会根据 flags 的**最终状态**重新应用所有差异。如果 flags 与真实窗口不一致,任何一次操作都会把窗口"纠正"到 flags 描述的状态,覆盖掉你用 Win32 直接做的更改。且 `apply_diff` 最后有 `SetWindowLongW` + `SetWindowPos(SWP_FRAMECHANGED)`,会刷新整个窗口样式,Win32 的手动修改基本都会被覆盖或产生冲突。

## 5. 易错点清单

| # | 场景 | 错误做法 | 正确做法 |
|---|------|----------|----------|
| 1 | 显示窗口 | `SetWindowPos(SWP_SHOWWINDOW)` | `window.show()` |
| 2 | 隐藏窗口 | `SetWindowPos(SWP_HIDEWINDOW)` | `window.hide()` |
| 3 | 隐藏后再显示+最大化 | 显示绕过 tao → 最大化被 SW_HIDE 覆盖 | 显示走 tao,再最大化 |
| 4 | 判断窗口可见性 | `IsWindowVisible(hwnd)`(真实状态) | `window.is_visible()`(flags 状态) |
| 5 | 最小化判断 | `IsIconic(hwnd)` | `window.is_minimized()` |
| 6 | 调整任务栏显示 | `SetWindowPos` 带 `SWP_SHOWWINDOW` | 只调样式 `SetWindowLongW` + 无 `SWP_SHOWWINDOW` 的 `SetWindowPos` |

> 一般原则:能走 tauri API 就不要直接调 Win32。Win32 仅用于 tauri 不提供的底层能力(如 `WS_EX_TOOLWINDOW` 扩展样式、`ITaskbarList`)。

## 6. 相关代码

- 修复文件:`app/src-tauri/src/shortcuts.rs`(`show_main_window` / `toggle_main_window`)
- 提交:`85d7c3f fix: 关闭后最大化/还原触发最小化问题`
- 关联文档:`docs/tauri-tray-taskbar.md`(托盘/任务栏控制,其中 `ITaskbarList` 与 `WS_EX_TOOLWINDOW` 方案与此文档配套)
- 日志记录:`app/logs.md`
