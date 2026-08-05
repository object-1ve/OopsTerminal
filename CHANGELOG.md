# Changelog

本项目所有值得记录的变更都会写入此文件。发布时,workflow 会自动提取当前版本号对应的章节作为 GitHub Release 正文。

## [Unreleased]

### Fixed
- 修复自定义字体加载超时:路径经过不受信任的 junction(如 Scoop 的 `current` 目录)时,后端会先解析链接到真实路径再加载,不再回退默认字体。
- 保存字体路径时在设置界面即时校验(解析链接、检查文件存在与格式),无效路径直接给出中文错误提示,不再静默存坏路径。

## [0.0.8] - 2026-08-03

### Added
- 设置中新增「终端字体文件」选项,可浏览并选择本地字体文件(ttf/otf/woff/woff2),留空使用默认字体。
- 支持实时切换字体:保存后已打开的终端立即应用新字体,无需重启。

### Changed
- 改用 Tauri 内置 asset 协议加载本地字体文件,替代自定义协议与 base64 IPC 方案,规避 CORS 与卡死问题。
- 字体加载失败或超时时优雅回退默认字体,避免终端卡在加载状态。
- 设置界面增加字体加载结果反馈(成功/失败提示信息)。

### Fixed
- 修复终端加载自定义字体时卡住的问题。

## [0.0.7] - 2026-08-01

### Fixed
- 修复 CJK/拉丁混合文字在终端中的宽度与对齐问题。
- 修复终端渲染错位:将 padding 移到 xterm 元素上,使 FitAddon 正确测量可用宽度。
- 修复终端启动时列/行数不准的问题:以前端计算出的行列数创建 PTY。
- 移除打包的内嵌等宽字体,改用系统默认等宽字体。

### Changed
- 启用 `allowProposedApi` 以支持 Unicode 11 宽度探测,正确渲染 emoji/图标。

## [0.0.6] - 2026-08-03

### Changed
- 标题栏按钮调整:移除关闭按钮,最小化按钮移至最右侧。

## [0.0.5] - 2026-07-31

### Fixed
- 修复托盘与任务栏图标显示/隐藏不生效的问题。

### Added
- 启用单应用锁,重复启动时唤起已有实例。
- 设置中新增托盘与任务栏图标显隐配置。

[Unreleased]: https://github.com/object-1ve/OopsTerminal/compare/v0.0.8...HEAD
[0.0.8]: https://github.com/object-1ve/OopsTerminal/compare/v0.0.7...v0.0.8
[0.0.7]: https://github.com/object-1ve/OopsTerminal/compare/v0.0.6...v0.0.7
[0.0.6]: https://github.com/object-1ve/OopsTerminal/compare/v0.0.5...v0.0.6
[0.0.5]: https://github.com/object-1ve/OopsTerminal/compare/v0.0.4...v0.0.5
