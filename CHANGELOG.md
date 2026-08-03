# Changelog

本项目所有值得记录的变更都会写入此文件。发布时,workflow 会自动提取当前版本号对应的章节作为 GitHub Release 正文。

## [Unreleased]

## [0.0.7] - 2026-08-03

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

[Unreleased]: https://github.com/object-1ve/OopsTerminal/compare/v0.0.7...HEAD
[0.0.7]: https://github.com/object-1ve/OopsTerminal/compare/v0.0.6...v0.0.7
[0.0.6]: https://github.com/object-1ve/OopsTerminal/compare/v0.0.5...v0.0.6
[0.0.5]: https://github.com/object-1ve/OopsTerminal/compare/v0.0.4...v0.0.5
