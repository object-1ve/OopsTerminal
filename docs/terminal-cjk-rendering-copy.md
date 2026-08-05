# 终端中文表格渲染与复制排查修复记录

> 适用:Tauri 2(WebView2/Chromium)+ xterm.js(`@xterm/xterm` v6)+ `portable_pty` 的终端应用。
> 来源:OopsTerminal 实战,针对含中文的表格(PowerShell 输出)在终端里"右边界贴边、中英文起始位置对不齐、复制中文少字"三个反馈的完整排查与验证。
> 与 `docs/xterm-terminal-rendering.md` 互补:那篇讲"行首字符跑到上一行行尾"的四个根因,这篇讲 CJK 表格的右边缘观感与复制链路验证。

## 目录

1. [问题现象](#1-问题现象)
2. [排查结论速览](#2-排查结论速览)
3. [问题一:表格右边界贴边/拥挤](#3-问题一表格右边界贴边拥挤)
4. [问题二:中文字符宽度/起始位置对齐](#4-问题二中文字符宽度起始位置对齐)
5. [问题三:复制中文丢字](#5-问题三复制中文丢字)
6. [验证方法:无头浏览器实测](#6-验证方法无头浏览器实测)
7. [最终代码状态](#7-最终代码状态)
8. [相关提交](#8-相关提交)

## 1. 问题现象

用户在终端(PowerShell 输出中文表格)反馈三个显示/交互问题:

1. **表格右边界截断感**:表格最右侧有一条竖线,内容与终端右侧边框/滚动条之间几乎没有留白,`$null)` 等内容紧贴边缘,视觉上"被夹住"。
2. **中文字符对齐偏差**:"不走" 和 "走系统代理" 的起始位置看起来有微妙错位;中英文混排处字符宽度计算可能不准。
3. **复制中文丢字**:选中含中文的文本复制后,粘贴会少一个字符。

## 2. 排查结论速览

| 反馈 | 结论 | 处理 |
| --- | --- | --- |
| 右边界贴边/拥挤 | `.xterm` 只设了左侧 padding,右边缘紧贴滚动条区域 | 增加右侧 6px padding,`4px 0 4px 6px` → `4px 6px` |
| 中文对齐偏差 | 三个历史根因(见下)已被既有修复覆盖 | 保持 Unicode11 + WebGL + `text-spacing-trim`,无需新改动 |
| 复制中文丢字 | **xterm 6 复制/粘贴全程走完整 JS 字符串,不存在字节截断** | 实测验证 100% 完整,无需代码改动 |

关键认知:第 3 条"复制丢字"是**误报/以讹传讹**。xterm 的选区文本由 `translateToString` 拼出完整字符串,`copyHandler` 用 `setData('text/plain', 完整字符串)` 写入剪贴板,全程没有按字节/按 UTF-8 边界切割的逻辑,中文字符(3 字节/字)不会被截断。真正会丢字的场景出现在**后端字节流按包转发**时(见 `terminal.rs` 已实现的 `from_utf8 + valid_up_to` 缓存逻辑),那是输入显示路径,不是复制路径。

## 3. 问题一:表格右边界贴边/拥挤

### 3.1 根因

`TerminalView.css` 中 `.xterm` 的 padding 是 `4px 0 4px 6px`(上 右 下 左),**右侧没有留白**。xterm 渲染网格到 `.xterm` 的 content 区,右边缘直接顶到滚动条区域,视觉上内容紧贴边框。

实测(1000px 容器,14px 字号,Consolas):

| 项 | 修改前 `4px 0 4px 6px` | 修改后 `4px 6px` |
| --- | --- | --- |
| 网格右边缘(距容器左) | 986px | 979px |
| 右侧留白(至容器边,含滚动条) | 14px | 21px |
| cols | 140 | 139 |

### 3.2 修复

```css
.terminal-container .xterm {
  height: 100%;
  /* 之前:4px 0 4px 6px(无右侧留白,行末内容贴边) */
  padding: 4px 6px;   /* 左右对称 6px,FitAddon 自动重算 cols */
  box-sizing: border-box;
}
```

要点:

- padding 必须在 `.xterm` 上,而不是父容器(`FitAddon.proposeDimensions` 只减 `.xterm` 自身 padding,见 `docs/xterm-terminal-rendering.md` 根因 2)。
- 右侧加 padding 后 FitAddon 会重新算出更小的 cols,内容按新列宽重排,**不会截断**,只是少一列。
- 中文字符双宽度渲染,行末留白对 CJK 表格观感提升明显。

## 4. 问题二:中文字符宽度/起始位置对齐

"不走" 与 "走系统代理" 起始位置偏差、中英文混排错位,是三个历史根因的叠加,已在既有提交中修复,本次确认无需新改动:

| 根因 | 修复 | 对应提交 |
| --- | --- | --- |
| Chromium 压缩 CJK 全角标点,宽度测量与渲染不一致 | `.xterm { text-spacing-trim: space-all }` | `2155551` / `a072232` |
| 默认 `UnicodeV6` 把 BMP 外字符(emoji)当 1 格 | `@xterm/addon-unicode11`,`term.unicode.activeVersion = "11"` | `0797a9c` |
| DOM 渲染器给宽字符 span 加 `letter-spacing`,Chromium 重复计算导致 CJK 行右边界漂移(xtermjs/xterm.js#6058) | `@xterm/addon-webgl`,失败回退 DOM | `1c61906` |

提示:排查这类问题时,先确认渲染器实际是哪个(WebGL 成功与否看控制台日志),再看宽字符测量。不要一上来换字体。

## 5. 问题三:复制中文丢字

### 5.1 结论

**xterm 6.0.0 的复制/粘贴链路对多字节 UTF-8 完全安全,实测无误。**

### 5.2 代码走查

- 选区文本:`SelectionService.selectionText` → `translateBufferLineToString` → `BufferLine.translateToString`,逐单元格把 UTF-32 codepoint 转回完整字符串,宽字符/emoji 都正确处理。
- 写入剪贴板:`browser/Clipboard.ts` 的 `copyHandler` 直接 `ev.clipboardData.setData('text/plain', selectionService.selectionText)`,**没有**按字节截断。
- 粘贴:`handlePasteEvent` → `ev.clipboardData.getData('text/plain')` → `prepareTextForTerminal`(只把 `\r?\n` 归一化成 `\r`)→ `coreService.triggerDataEvent`。全程是 JS 字符串,UTF-8 编码发生在写 PTY 时由 Rust 侧 `String::as_bytes()` 一次性完成,不拆包。

### 5.3 真正的字节边界风险在哪里

后端把 PTY 输出转发给前端时按 8KB 分块,可能**切在多字节字符中间**。`terminal.rs` 已正确处理:用 `from_utf8` + `valid_up_to()` 缓存不完整尾部,凑齐完整序列再 emit,避免跨包乱码/丢字。这条链路是显示路径,与复制无关,但排查"终端乱码"时优先查这里。

## 6. 验证方法:无头浏览器实测

用 `puppeteer-core`(系统 Chrome)+ 本地 Vite 页面复现,全部用**文本断言**验证,不依赖截图:

1. **布局测量**:读 `.xterm-screen` 的 `getBoundingClientRect`,对比容器右缘,算出右侧留白像素。
2. **复制**:`term.selectLines(1,1)` 后 `term.getSelection()`,断言中文行 `│ 名称 │ 状态 │ 值 │` 逐字完整;`navigator.clipboard.writeText` 后外部粘贴对照。
3. **粘贴**:分别用 `term.paste(text)` 和真实 `ClipboardEvent` 注入,断言 `term.onData` 收到的字符串与输入逐字符相等。

实测数据(示例):输入 `"中文测试 abc 中文测试"`(13 个码点),`onData` 输出 13 个码点,`一致=true`。

## 7. 最终代码状态

- `app/src/components/TerminalView.css` — `.xterm` padding `4px 6px`(右边界留白)
- `app/src/components/TerminalView.tsx` — `allowProposedApi`、Unicode11、WebGL(失败回退)、fit 时序、`create_terminal` 传实际尺寸
- `app/src-tauri/src/terminal.rs` — `create_terminal(cols, rows)`、UTF-8 分包缓存

## 8. 相关提交

| Commit | 说明 |
| --- | --- |
| 本次(文档 + 右侧留白) | `.xterm` 增加右侧 padding,新增本文档 |
| `1c61906` | 启用 WebGL 渲染器修复中文行右边框错位(xtermjs/xterm.js#6058) |
| `a072232` / `2155551` | `text-spacing-trim: space-all` 修复 CJK 标点压缩错位 |
| `0797a9c` / `6a312d3` | Unicode 11 宽度检测 + `allowProposedApi` |
| `5b30d1f` / `7dfa19c` | fit 紧贴 create_terminal,PTY 尺寸一致 |
| `2e2a093` | padding 移到 `.xterm` 上,FitAddon 测量正确 |
