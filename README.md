# OopsTerminal

> 基于 Tauri 2 + React 19 + xterm.js 的轻量级桌面终端,可隐藏在托盘、通过全局快捷键随时唤起。

![GitHub release](https://img.shields.io/github/v/release/object-1ve/OopsTerminal)

OopsTerminal 是一款 Windows 优先的桌面终端应用。它用 Rust + Tauri 2 提供原生壳(ConPTY 拉起 PowerShell),用 React + TypeScript + Vite 构建界面,xterm.js 负责终端渲染。窗口默认不出现在任务栏,关闭即隐藏到托盘,配合可配置的全局快捷键,适合作为随叫随到的快速终端。

## 特性

### 🖥️ 终端能力
- **多标签终端** — 顶部标签栏 + 底部终端区域,可新建 / 关闭多个会话,关闭标签时自动终止对应 shell 进程
- **xterm.js 渲染** — VS Code 同款渲染引擎,启用 WebGL 渲染器;完整 ANSI / 256 色 / 光标控制支持
- **CJK 中文渲染** — 启用 Unicode 11 宽度探测,emoji 与中文对齐正确;WebGL 渲染器按网格绘制字形,修复含中文行右边框错位
- **portable-pty 桥接** — Rust 侧通过 ConPTY 拉起 `powershell.exe`(Windows),字节流双向转发并按 UTF-8 安全分包,避免跨包截断乱码
- **右键即复制** — 右键复制选中内容(无选区时选中光标处单词),行为对齐 PowerShell:复制完成后清除选区高亮
- **自定义启动目录** — 可为新终端设置默认工作目录,留空使用用户主目录

### ⚙️ 设置(保存到本地 SQLite)
- **全局快捷键** — 可配置「显示/隐藏窗口」与「退出程序」两个全局快捷键,录制时自动生成 Tauri 加速键格式,冲突时保留旧设置
- **自定义字体** — 支持 ttf / otf / woff / woff2 本地字体文件,保存后已打开的终端实时应用;加载失败或超时优雅回退默认字体
- **托盘与任务栏图标** — 独立控制托盘图标与任务栏按钮的显隐,改动立即生效

### 🎛️ 窗口与系统集成
- **隐藏到后台** — 关闭窗口时隐藏而非退出,可通过全局快捷键或托盘菜单随时唤出
- **单实例锁** — 重复启动时唤起已有实例,而不是再开一个进程
- **系统托盘** — 左键单击切换窗口显隐,托盘菜单提供「显示/隐藏窗口」与「退出」
- **任务栏图标隐藏** — 通过 `WS_EX_TOOLWINDOW` + `ITaskbarList::AddTab/DeleteTab` 双保险实现
- **置顶按钮** — 标题栏图钉按钮可将窗口置顶
- **无边框窗口** — 自定义标题栏(置顶 / 设置 / 最大化 / 最小化),每次启动自动居中

## 技术栈

| 层 | 技术 |
|----|------|
| 桌面框架 | [Tauri 2](https://tauri.app) + Rust |
| 前端 | React 19 + TypeScript + Vite |
| 终端渲染 | [xterm.js](https://xtermjs.org) + Fit / Unicode 11 / WebGL 插件 |
| PTY 桥接 | [portable-pty](https://github.com/wez/wezterm/tree/main/pty)(Windows 走 ConPTY) |
| 设置存储 | rusqlite(SQLite) |
| 插件 | 全局快捷键、单实例、文件对话框、日志 |

## 目录结构

```
├── app/                          # 应用主体
│   ├── src/                      # React 前端源码
│   │   ├── App.tsx               # 主应用(标签页管理)
│   │   └── components/           # 标题栏 / 标签栏 / 终端 / 设置弹窗
│   └── src-tauri/                # Tauri Rust 后端
│       ├── src/
│       │   ├── lib.rs            # 应用装配(插件、数据库、托盘、命令注册)
│       │   ├── terminal.rs       # PTY 会话管理(创建/写入/调整尺寸/终止)
│       │   ├── shortcuts.rs      # 全局快捷键与字体路径解析
│       │   ├── ui.rs             # 托盘与任务栏图标控制
│       │   └── db.rs             # SQLite 设置读写
│       ├── tauri.conf.json       # Tauri 配置(窗口、打包目标、CSP)
│       └── capabilities/         # 权限配置
├── .github/workflows/            # GitHub Actions(打标签自动构建发布)
└── CHANGELOG.md                  # 变更记录
```

## 环境要求

- [Node.js](https://nodejs.org) 20+ 与 [pnpm](https://pnpm.io)(构建 CI 使用 pnpm)
- [Rust](https://www.rust-lang.org) stable(项目要求 1.77.2+)
- Windows 平台还需 [WebView2](https://developer.microsoft.com/en-us/microsoft-edge/webview2/)(Windows 11 自带)

## 快速开始

```bash
cd app

# 安装依赖
pnpm install

# 启动开发模式(前端 + Tauri 桌面窗口)
pnpm tdev

# 仅前端开发(浏览器预览)
pnpm dev

# 构建生产安装包(Windows MSI)
pnpm tbuild --bundles msi
```

> 开发模式前端端口见 `src-tauri/tauri.conf.json` 的 `devUrl`(默认 `http://localhost:1420`)。

## 常用脚本

| 命令 | 说明 |
|------|------|
| `pnpm dev` | 启动 Vite 前端开发服务器 |
| `pnpm build` | 构建前端代码(TypeScript 检查 + Vite 打包) |
| `pnpm tdev` | 启动 Tauri 开发模式(桌面窗口) |
| `pnpm tbuild` | 构建 Tauri 生产安装包 |
| `pnpm lint` | ESLint 代码检查 |
| `cd src-tauri && cargo test` | 运行 Rust 单元测试 |

## 测试

```bash
cd app/src-tauri
cargo test
```

测试覆盖设置读写(`db::tests`)与快捷键 / 字体路径解析(`shortcuts::tests`),其中 junction 解析测试会真实创建一个目录联接验证链接解析。

## 打包与发布

- **Windows**:MSI 安装包,WiX 语言为简体中文(`zh-CN`)
- **macOS**:DMG(未签名,发布如需 Gatekeeper 干净分发请自行配置签名)
- 推送 `v*` 标签(如 `v0.0.9`)即触发 [Build and Release](.github/workflows/release.yml) 工作流:自动把标签版本同步进 `package.json` / `tauri.conf.json` / `Cargo.toml`,跑单元测试,构建安装包并从 git 历史生成 Release 正文

## 设置说明

设置项保存在应用数据目录的 `oops_terminal.db`(SQLite)中:

| 设置项 | 说明 |
|--------|------|
| 显示/隐藏窗口快捷键 | 全局快捷键,留空禁用 |
| 退出程序快捷键 | 全局快捷键,留空禁用 |
| 终端默认启动路径 | 新终端的工作目录,留空使用用户主目录 |
| 终端字体文件 | ttf / otf / woff / woff2 本地文件,留空使用默认字体 |
| 显示托盘图标 | 默认开启 |
| 显示任务栏图标 | 默认关闭 |

> 若通过浏览按钮选择 Scoop 等目录中的字体提示"无法访问"(不受信任的装入点),可直接手动输入完整路径:应用会自动把 `current` junction 解析到真实目录后再加载字体。

## 相关文档

- [CHANGELOG.md](CHANGELOG.md) — 版本变更记录

## 许可

本项目尚未指定开源许可,请先与维护者确认再用于其他场景。
