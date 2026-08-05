# Tauri 2 + xterm.js 应用本地字体文件技术方案

> 适用:Tauri 2(WebView2/Chromium)+ xterm.js (`@xterm/xterm` v6)的桌面终端应用。
> 来源:OopsTerminal 实战(设置 → 终端字体文件 → 选择本地 ttf/otf/woff/woff2 → 终端实时应用),经真机验证。

## 目录

1. [需求与目标](#1-需求与目标)
2. [方案演进:四个尝试与失败教训](#2-方案演进四个尝试与失败教训)
3. [最终方案:asset 协议 + FontFace](#3-最终方案asset-协议--fontface)
4. [核心代码](#4-核心代码)
5. [为什么 PowerShell 切换字体"瞬间生效"](#5-为什么-powershell-切换字体瞬间生效)
6. [易错点清单](#6-易错点清单)
7. [相关提交](#7-相关提交)

## 1. 需求与目标

用户希望在终端应用里**使用任意本地字体文件**(不打包进应用),例如:

- 等宽 CJK 字体(Sarasa Mono SC、等距更纱黑体等),保证中英文混排对齐
- 个人喜欢的编程字体(JetBrains Mono、Fira Code 等)

要求:

1. 在设置界面填写/浏览选择本地字体文件路径
2. 应用启动时自动加载该字体
3. 设置变更后**已打开的终端实时切换**字体
4. 字体加载失败时**优雅回退默认字体**,不能卡死终端
5. 路径持久化,重启后仍生效

## 2. 方案演进:四个尝试与失败教训

### 2.1 方案 A:打包字体进应用(@font-face)❌

把字体文件放进 `public/fonts/`,用 `@font-face` 内嵌,并作为 xterm `fontFamily` 首位。

**问题:**
- 字体文件体积大(Sarasa Mono SC 14MB),打包进应用代价高
- `font-display: swap` 下 xterm 可能在字体就绪前测量网格,换入后宽度缓存与真实渲染不一致
- 无法满足"用户自选任意字体"的需求

**结论:否决。** 先排查尺寸/测量类根因,不要一上来就打包字体(见 `xterm-terminal-rendering.md`)。

### 2.2 方案 B:CSS font-family 名称(填字体名)❌

设置界面填 CSS 字体族名(如 `"Cascadia Mono"`),直接设置 `term.options.fontFamily`。

**问题:**
- 只能使用**系统已安装**的字体,无法加载任意文件
- 用户需要知道准确的字体名,还要自己安装字体,体验差
- 填错名字静默回退,没有反馈

**结论:否决。** 用户明确要求"填写本地字体的路径"。

### 2.3 方案 C:base64 走 IPC 传字体文件 ❌

后端 `read_font_file` 命令读取文件,base64 编码后经 Tauri IPC 返回前端,前端塞进 `data:` URL 给 `FontFace`。

**问题(致命):**
- 14MB 字体 base64 后约 19MB JSON 字符串,WebView2 的 IPC 处理挂起
- `FontFace.load()` 无超时保护,前端一直卡在加载画面

**结论:否决。** 大文件永远不要走 IPC 传 base64。

### 2.4 方案 D:自定义 `font://` 协议 ❌

Rust 注册 `register_uri_scheme_protocol("font", ...)` 流式返回字体字节,前端 `url(font://localhost/<base64路径>)`。

**问题(致命):**
- 自定义协议的响应**缺少 CORS 头**,Chromium 把字体请求当跨源请求直接阻止
- `FontFace.load()` 挂起直到超时(用户看到"字体加载超时"提示)
- 路径 base64 编码、URL 转义等细节容易踩坑

**结论:否决。** 不要自己实现协议,Tauri 内置的 asset 协议已经处理好了 CORS。

### 2.5 最终方案:asset 协议 + FontFace ✅

见下一节。

## 3. 最终方案:asset 协议 + FontFace

Tauri 内置 **asset 协议**(`asset://localhost/<path>`),用于从本地文件系统加载任意文件到 WebView:

- 由 Tauri 官方实现,响应自带正确的 CORS 头,浏览器放行
- 流式读取,不经 IPC,大文件(几十 MB)也不卡 UI
- 通过 `convertFileSrc(path)` 生成 URL,自动处理路径转义

流程:

```
用户选择字体文件 → 路径存 DB(settings 表 terminal_font_path)
     ↓
应用启动 / 设置变更(settings-changed 事件)
     ↓
前端 resolveTerminalFont():
  1. invoke("get_settings") 读路径
  2. new FontFace("OopsTerminalCustomFont", `url(${convertFileSrc(path)})`)
  3. face.load() + 8s 超时保护
  4. document.fonts.add(loaded)
  5. 返回 font-family 字符串 → term.options.fontFamily
     ↓
字体变化 → fit.fit() + resize_terminal 同步 PTY 尺寸
```

## 4. 核心代码

### 4.0 特殊场景:路径经过不受信任的 junction(Scoop `current`)⚠️

用 Scoop 安装字体时,`C:\Users\<user>\scoop\apps\<app>\current` 是一个指向版本号目录的 **junction**。Windows 会把它视为「不受信任的装入点」,**拒绝遍历**该链接——文件管理器、`File::open`、asset 协议全部读不到 `current` 下的文件,`FontFace.load()` 挂起直到超时,终端回退默认字体。

判断标准:`Test-Path ...\current\xxx.ttf` 返回 `False`,但 `...\<版本号>\xxx.ttf` 正常;`readlink`/PowerShell `(Get-Item ...\current).Target` 仍能读出链接指向(读取链接数据不需要遍历)。

**修复:后端逐组件解析链接,把 `current` 解析成真实版本目录后再加载。**

```rust
// 关键点:
// 1. 用 symlink_metadata 判断 reparse point(不要用 metadata,它会跟随链接被拒绝)
// 2. fs::read_link 读取指向(不遍历,不受「不受信任装入点」影响)
// 3. 去掉 read_link 返回的 \\?\ 前缀,拼出真实路径
fn resolve_font_path(raw: &str) -> Result<PathBuf, String> { ... }
```

前端加载前先调用后端命令 `resolve_terminal_font_path` 拿到真实路径,再 `convertFileSrc` + `FontFace`;保存设置时后端也先解析校验,路径不可用当场报中文错误,不再静默存坏路径。

> 另外,Scoop 安装的字体通常也已注册到系统(如 `C:\Users\<user>\AppData\Local\Microsoft\Windows\Fonts\`),可引导用户去那里选文件,完全绕开 junction。

### 4.1 tauri.conf.json:开启 asset 协议

```json
{
  "app": {
    "security": {
      "csp": null,
      "assetProtocol": {
        "enable": true,
        "scope": ["**"]
      }
    }
  }
}
```

> `scope: ["**"]` 允许读取任意路径。如需更严格,可限制为字体目录,例如 `["C:/Fonts/**"]`。
> 无需在 capabilities 中额外加权限,`core:default` 已覆盖。

### 4.2 前端:加载并应用字体

```typescript
import { convertFileSrc, invoke } from "@tauri-apps/api/core";

const CUSTOM_FONT_FAMILY = "OopsTerminalCustomFont";
const FONT_LOAD_TIMEOUT = 8000; // 8s 超时,回退默认字体

function withTimeout<T>(promise: Promise<T>, ms: number, fallback: T): Promise<T> {
  return new Promise<T>((resolve) => {
    const timer = setTimeout(() => resolve(fallback), ms);
    promise.then(
      (v) => { clearTimeout(timer); resolve(v); },
      () => { clearTimeout(timer); resolve(fallback); },
    );
  });
}

async function resolveTerminalFont(): Promise<string> {
  const s = await invoke<Settings>("get_settings");
  const path = s.terminal_font_path?.trim();
  if (!path) return DEFAULT_FONT;

  // 关键:asset 协议 URL 由 convertFileSrc 生成,自带 CORS 处理
  const face = new FontFace(CUSTOM_FONT_FAMILY, `url(${convertFileSrc(path)})`);
  const loaded = await withTimeout(face.load(), FONT_LOAD_TIMEOUT, null);
  if (!loaded) return DEFAULT_FONT; // 失败回退,不卡终端

  document.fonts.add(loaded);
  return `"${CUSTOM_FONT_FAMILY}", ${DEFAULT_FONT}`;
}
```

### 4.3 应用字体后必须重新 fit

字体变化会改变字符宽度,必须重新计算 cols/rows 并同步给后端 PTY:

```typescript
function applyTerminalFont(term, fit, font, doResize) {
  if (term.options.fontFamily !== font) {
    term.options.fontFamily = font;
    fit.fit();       // 按新字体宽度重新计算行列数
    doResize();      // resize_terminal 同步给后端
  }
}
```

### 4.4 设置变更实时生效

后端保存字体路径后广播事件,所有已打开的终端监听并重新加载:

```rust
// Rust:set_terminal_font_path 保存后
let _ = app.emit("settings-changed", ());
```

```typescript
// 前端
listen("settings-changed", () => {}).then((un) => {
  resolveTerminalFont().then((font) => applyTerminalFont(term, fit, font, doResize));
});
```

### 4.5 设置界面验证 + 用户反馈

保存时用同一个加载逻辑验证字体,给出绿色/红色提示,并打 console 日志:

```typescript
// 成功 → 绿色提示:"字体已应用成功: C:\Fonts\xxx.ttf"
// 失败 → 红色提示:"字体路径已保存,但字体加载超时...终端将使用默认字体"
```

## 5. 为什么 PowerShell 切换字体"瞬间生效"

PowerShell / Windows Terminal 切换的是**系统已安装字体**:

- 字体文件在系统启动或首次使用时已加载进内存(字体缓存)
- 切换只是改一个渲染参数(GDI/DirectWrite 的字体名解析),不读文件

而我们的方案:

- 从磁盘读取任意**用户指定**的本地文件(可能 10-20MB)
- 解析字体 → 注册 FontFace → 浏览器加载
- xterm 重新测量每个字符的宽度 → 重新 fit → 同步 PTY

首次加载有真实开销,但正常远小于 8 秒超时。**性能优化方向**(如需要):
- `document.fonts` 缓存:同路径字体只注册一次
- 预加载:应用启动时后台提前加载,终端打开时零等待
- 路径不变时跳过重新加载

## 6. 易错点清单

- [x] **不要**用 base64 走 IPC 传大字体文件,会卡死 WebView2
- [x] **不要**自己注册自定义协议返回字体,响应缺 CORS 头会被 Chromium 阻止
- [x] 用 `convertFileSrc(path)` + `assetProtocol` 配置,官方实现处理了 CORS
- [x] `FontFace.load()` 必须有超时保护,否则终端卡在加载画面
- [x] 任何失败路径都要回退默认字体,不能让终端不可用
- [x] 字体应用后必须 `fit.fit()` + 同步 PTY 尺寸,否则列数错位(见 `xterm-terminal-rendering.md`)
- [x] 设置保存后广播 `settings-changed`,已打开的终端才能实时切换
- [x] 给用户可见反馈(设置界面 message + console 日志),否则"静默失败"很难排查
- [x] 路径持久化到 DB,启动时重新加载

## 7. 相关提交

| Commit | 说明 |
| --- | --- |
| `9442026` | **最终修复**:改用 asset 协议(convertFileSrc + assetProtocol),删除自定义 font:// 协议 |
| `91a6d31` | 字体应用反馈:设置界面绿色/红色提示 + console 日志 |
| `d4f85c6` | 自定义 font:// 协议流式加载 + 8s 超时回退(后因 CORS 被 9442026 替换) |
| `54a37ab` | 字体设置改为本地文件路径 + 浏览按钮 + read_font_file(base64 方案,后被替换) |
| `4f47424` | 字体设置初版(CSS font-family 名方案,后被替换) |

## 附:最终关键文件

- `app/src-tauri/tauri.conf.json` — `assetProtocol.enable + scope`
- `app/src/components/TerminalView.tsx` — `resolveTerminalFont` / `applyTerminalFont` / 事件监听
- `app/src/components/SettingsModal.tsx` — 路径输入 + 浏览 + 保存验证反馈
- `app/src-tauri/src/shortcuts.rs` — `set_terminal_font_path` 保存 + 广播事件
- `app/src-tauri/src/db.rs` — `terminal_font_path` 字段持久化
