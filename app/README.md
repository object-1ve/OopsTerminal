# OopsTerminal

> 基于 Tauri 2 + React 19 + TypeScript + Vite 构建的桌面应用模板。

## 特性一览

### 🖥️ 桌面端核心
- **Tauri 2 框架** — 使用 Rust 构建原生桌面应用，支持 Windows / macOS / Linux 三端，应用体积小、性能高、内存占用低
- **多标签终端** — 上下结构:顶部标签栏 + 底部 PowerShell 终端，可新建/关闭多个会话
- **xterm.js 渲染** — VS Code 同款终端渲染引擎，完整 ANSI/256 色/光标控制支持
- **portable-pty 桥接** — Rust 侧 ConPTY 拉起 powershell.exe，字节流双向转发（UTF-8 安全分包）
- **隐藏到后台** — 关闭窗口时自动隐藏而非退出，可通过托盘菜单随时唤出
- **任务栏图标隐藏** — 窗口不出现在任务栏（WS_EX_TOOLWINDOW）
- **置顶功能** — 右上角图钉按钮可将窗口置顶
- **窗口定制** — 自定义窗口标题为 "OopsTerminal"，初始尺寸 800×600，支持窗口缩放，每次启动自动居中显示

### ⚛️ 前端技术栈
- **React 19** — 最新版 React，支持 Server Components、Actions、新 Hooks 等特性
- **TypeScript 6** — 全量 TypeScript 类型检查，提升代码健壮性和开发体验
- **Vite 8** — 极速开发服务器（端口 3000），毫秒级 HMR 热更新
- **HMR 热模块替换** — 保存代码后即时生效，无需手动刷新

### 🎨 UI / 样式
- **深色/浅色自适应主题** — 跟随操作系统 `prefers-color-scheme` 自动切换，暗色模式下视觉优化
- **响应式布局** — 自适应桌面端与移动端尺寸，1024px 断点处调整排版
- **SVG Sprite 图标系统** — 使用 SVG `<use>` 引用矢量图标，轻量且可缩放
- **现代化字体栈** — `system-ui` 系统字体 + `ui-monospace` 等宽字体

### 🔧 开发工具链
- **ESLint + TypeScript** — 集成 `typescript-eslint`、`eslint-plugin-react-hooks`、`eslint-plugin-react-refresh`，确保代码质量
- **开发日志** — Debug 模式下集成 `tauri-plugin-log`，日志级别为 Info，方便调试
- **Rust 后端** — 基于 Tauri 2 的 Rust 后端架构，可编写高性能原生功能

### 📦 构建与分发
- **全平台打包** — 支持通过 `tauri build` 构建 Windows (.msi/.exe)、macOS (.dmg)、Linux (.deb/.AppImage) 原生安装包
- **多尺寸应用图标** — 内置 32×32、128×128、128×128@2x、ICNS、ICO 等多格式图标
- **CSP 安全策略** — 支持自定义 Content Security Policy（当前为宽松模式，可按需收紧）

## 快速开始

```bash
# 安装依赖
npm install

# 启动开发模式（前端 + Tauri 桌面窗口）
npm run tdev

# 构建生产版本
npm run tbuild

# 仅前端开发（浏览器预览）
npm run dev
```

## 项目结构

```
├── src/                  # React 前端源码
│   ├── App.tsx           # 主应用组件
│   ├── App.css           # 主应用样式
│   ├── index.css         # 全局样式 / CSS 变量 / 主题
│   └── main.tsx          # 应用入口
├── src-tauri/            # Tauri Rust 后端
│   ├── src/
│   │   ├── lib.rs        # 核心逻辑（托盘、窗口事件、日志）
│   │   └── main.rs       # 程序入口
│   ├── Cargo.toml        # Rust 依赖
│   ├── tauri.conf.json   # Tauri 配置
│   └── capabilities/     # 权限配置
├── public/               # 静态资源
├── dist/                 # 构建输出
├── vite.config.ts        # Vite 配置
├── eslint.config.js      # ESLint 配置
└── tsconfig*.json        # TypeScript 配置
```

## 脚本命令

| 命令 | 说明 |
|------|------|
| `npm run dev` | 启动 Vite 前端开发服务器 |
| `npm run build` | 构建前端代码 |
| `npm run tdev` | 启动 Tauri 开发模式（桌面窗口） |
| `npm run tbuild` | 构建 Tauri 生产安装包 |
| `npm run lint` | ESLint 代码检查 |
| `npm run preview` | 预览构建产物 |
