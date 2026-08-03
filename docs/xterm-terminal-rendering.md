# xterm.js 终端渲染错位排查与修复方案

> 适用:Tauri 2(WebView2/Chromium)+ xterm.js (`@xterm/xterm` v6)+ `portable_pty` 的终端应用。
> 来源:OopsTerminal 实战修复,经真机验证已解决("一行的前几个字符跑到上一行行尾"、右边缘截断、emoji/图标错位)。

## 目录

1. [问题现象](#1-问题现象)
2. [根本原因:四个独立问题叠加](#2-根本原因四个独立问题叠加)
3. [问题速查表](#3-问题速查表)
4. [修复方案详解](#4-修复方案详解)
5. [字体走过的弯路](#5-字体走过的弯路)
6. [易错点清单](#6-易错点清单)
7. [相关提交](#7-相关提交)

## 1. 问题现象

用户在终端里看到:

1. **每行开头的几个字符跑到上一行的末尾**(最严重,启动即出现)
2. 行末字符被截断 / 换行错位,中英文混排时更明显
3. 含 emoji/图标(如 `🐕` `✓` `▶`)的行右边缘溢出
4. 对比:原生 Windows Terminal / PowerShell 显示完全正常

## 2. 根本原因:四个独立问题叠加

### 2.1 PTY 初始尺寸与前端渲染尺寸不一致(现象 1 的根因)

后端创建 PTY 时硬编码了 `PtySize { rows: 30, cols: 100 }`,而前端 xterm 根据容器实际大小算出的可能是 `cols: 80`。shell(PowerShell)启动后按 100 列输出 prompt 并自行换行,前端却按 80 列渲染,行首字符就被"卷"到了上一行行尾。

PowerShell/Windows Terminal 没有此问题,因为原生 PTY 在创建时就知道正确尺寸,不存在尺寸不一致。

```rust
// 错误:硬编码
.openpty(PtySize { rows: 30, cols: 100, pixel_width: 0, pixel_height: 0 })

// 正确:由前端传入实际尺寸
.openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
```

### 2.2 FitAddon 测量的 padding 与真实渲染不一致(现象 1 的另一根因)

`FitAddon.proposeDimensions()` 计算可用宽度时:

```js
const parentElementWidth = parseInt(getComputedStyle(term.element.parentElement).width);
const elementPadding = getComputedStyle(term.element).padding;  // 减的是 .xterm 自身的 padding
const availableWidth = parentElementWidth - elementPaddingHor - scrollbarWidth;
```

如果 padding 写在**父容器**(`.terminal-container`)上而 `.xterm` 自身 padding 为 0,FitAddon 认为可用宽度 = 容器宽,但真实渲染从父容器 padding 内侧开始,实际可用宽度少了 6px → `cols` 偏大 → 行末放不下被折行。

### 2.3 Chromium 压缩 CJK 标点(现象 1 的第三个根因)

Chromium 默认对 CJK 全角标点(`。` `，` `：` `;` 等)做 **text-spacing-trim** 压缩。xterm 的 DOM 渲染器先按"标点占满 1 格"测量并计算 letter-spacing,真实渲染时标点却被压缩变窄,导致宽度测量与渲染不一致,行首字符被挤到上一行行尾。原生终端不受浏览器排版影响,所以正常。

### 2.4 emoji/图标宽度误判(现象 3 的根因)

xterm 默认的 `UnicodeV6.wcwidth` 对 **BMP 外字符**(U+10000+,含全部 emoji)一律返回宽度 1:

```typescript
// @xterm/xterm common/input/UnicodeV6.ts
public wcwidth(num: number): UnicodeCharWidth {
  // ...
  return 1;  // ← U+1F000+ 的 emoji 全部返回 1
}
```

但浏览器用 Segoe UI Emoji 渲染时占 **2 格宽**。缓冲区按 1 格记账、渲染占 2 格 → 行末溢出、后续列错位。

## 3. 问题速查表

| 现象 | 根因 | 修复 |
| --- | --- | --- |
| 首字符跑到上一行行尾(启动即有) | PTY 硬编码 100×30 | 前端 fit 后把 cols/rows 传入 `create_terminal` |
| 首字符跑到上一行行尾(所有行) | padding 写在父容器 | padding 移到 `.xterm` 元素上 |
| 首字符跑到上一行行尾(含中文标点) | Chrome text-spacing-trim 压缩标点 | `.xterm { text-spacing-trim: space-all }` |
| 含 emoji/图标的行右边缘溢出 | wcwidth 把 emoji 当 1 格 | 启用 `@xterm/addon-unicode11` |

## 4. 修复方案详解

### 4.1 前端:创建 PTY 前先 fit,传入实际尺寸

```typescript
// 关键:fit 必须紧贴 create_terminal,中间不要夹 await listen(...)!
// 否则 await 期间事件循环可能触发 ResizeObserver,尺寸已过期。
try {
  fit.fit();
} catch {
  /* 容器尚未有尺寸,等激活时再 fit */
}
const id = await invoke<number>("create_terminal", {
  cols: term.cols || 80,
  rows: term.rows || 24,
});
```

### 4.2 CSS:padding 放到 .xterm 上

```css
.terminal-container {
  padding: 0;           /* 父容器不再设 padding */
}

.terminal-container .xterm {
  height: 100%;
  padding: 4px 0 4px 6px;   /* padding 必须在 .xterm 上,FitAddon 才会正确扣除 */
  box-sizing: border-box;
}
```

### 4.3 CSS:禁用 CJK 标点压缩

```css
/*
 * text-spacing-trim 是继承属性,一处设置即可覆盖内部宽度测量容器。
 * 仅 Chromium 默认压缩,Firefox/Safari 无此问题。
 */
.terminal-container .xterm {
  text-spacing-trim: space-all;
}
```

### 4.4 Unicode 11 宽度检测

```bash
npm install @xterm/addon-unicode11
```

```typescript
import { Unicode11Addon } from "@xterm/addon-unicode11";

const term = new Terminal({
  allowProposedApi: true,  // 必须!unicode API 是 proposed API,不开会抛错
  // ...
});
const unicode11 = new Unicode11Addon();
term.loadAddon(unicode11);
term.unicode.activeVersion = "11";  // 切换 Unicode 版本(注意不是 setVersion)
```

Unicode 11 正确将 emoji(U+1F000+)识别为 2 格宽,与浏览器渲染一致。

## 5. 字体走过的弯路

为修复中英文混排错位,曾尝试**打包自洽等宽字体**(Sarasa Mono SC, 14MB):

- 用 `@font-face` 内嵌 + `font-display: swap`,并把 Sarasa 放在 fontFamily 首位
- 结论:**不是解决方案**,已回退

原因:

1. `font-display: swap` 下,xterm 可能在字体加载完成前就测量了网格(用 fallback 字体的宽度),字体换入后宽度缓存与真实渲染不一致
2. 即使等 `document.fonts.ready` 再建终端,也只解决了测量时序,emoji/PTY 尺寸等其他根因仍在
3. 14MB 字体包代价大,而系统字体(Consolas → 中文字体回退)配合上述 4 个修复已完全够用

教训:先排查尺寸/测量类根因,不要一上来就换字体。

## 6. 易错点清单

- [x] `create_terminal` 的 `fit.fit()` 必须紧贴调用,中间不要有 `await` 其他异步操作
- [x] `FitAddon` 只减 `.xterm` 自身的 padding,不要给父容器设 padding
- [x] WebView2(Chromium)必须加 `text-spacing-trim: space-all`,否则中文标点错位
- [x] 用 `@xterm/addon-unicode11` 时必须同时设 `allowProposedApi: true`(否则运行时抛错)
- [x] 切换 Unicode 版本用 `term.unicode.activeVersion = "11"`,不是 `setVersion()`
- [x] 别为宽度问题打包字体,先查尺寸测量链路

## 7. 相关提交

以下为 `72d9a91c`(调整标题栏按钮)之后所有与终端渲染相关的提交:

| Commit | 说明 |
| --- | --- |
| `6a312d3` | Add allowProposedApi: true for Unicode 11 addon |
| `0797a9c` | Add Unicode 11 width detection for correct emoji/icon rendering |
| `5b30d1f` | Move fit.fit() immediately before create_terminal to prevent stale cols/rows |
| `7dfa19c` | Fix terminal display: pass frontend-calculated cols/rows to PTY at creation time |
| `2e2a093` | Fix terminal misalignment: move padding to xterm element so FitAddon measures available width correctly |
| `a072232` | Restore text-spacing-trim fix for CJK punctuation misalignment |
| `f534285` | Remove bundled mono font, use system default font |
| `fff67f9` | Fix mixed CJK/Latin terminal width with bundled mono font(弯路,后被 f534285 回退) |

前置相关修复(在 `72d9a91c` 之前,背景参考):

| Commit | 说明 |
| --- | --- |
| `2155551` | fix: 修复终端 CJK 标点渲染错位(text-spacing-trim 首次引入) |
| `579ee97` / `2cd96b3` | 滚动条宽度样式(非错位问题) |

## 附:最终关键代码状态

- `app/src/components/TerminalView.tsx` — xterm 初始化(allowProposedApi、Unicode11、fit 时序、create_terminal 传尺寸)
- `app/src/components/TerminalView.css` — `.xterm` padding、text-spacing-trim、滚动条
- `app/src-tauri/src/terminal.rs` — `create_terminal(cols, rows)` 使用前端尺寸创建 PTY
