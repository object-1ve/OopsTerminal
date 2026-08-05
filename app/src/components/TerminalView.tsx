import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebglAddon } from "@xterm/addon-webgl";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "@xterm/xterm/css/xterm.css";
import "./TerminalView.css";

type TerminalOutput = { id: number; data: string };
type TerminalExit = { id: number };
type Settings = { terminal_font_path: string | null };

/** 默认终端字体。 */
const DEFAULT_FONT = 'Consolas, "Cascadia Mono", "Courier New", monospace';

/** 注册到页面里的自定义字体家族名。 */
const CUSTOM_FONT_FAMILY = "OopsTerminalCustomFont";

/** 字体加载超时(毫秒),超时回退默认字体,避免终端卡在加载。 */
const FONT_LOAD_TIMEOUT = 8000;

/** 给 Promise 加超时,超时返回 fallback。 */
function withTimeout<T>(promise: Promise<T>, ms: number, fallback: T): Promise<T> {
  return new Promise<T>((resolve) => {
    const timer = setTimeout(() => resolve(fallback), ms);
    promise.then(
      (v) => {
        clearTimeout(timer);
        resolve(v);
      },
      () => {
        clearTimeout(timer);
        resolve(fallback);
      },
    );
  });
}

/** 字体解析结果。 */
type FontResolve = {
  /** 实际应用到终端的 font-family 值。 */
  font: string;
  /** 是否成功应用了自定义字体。 */
  applied: boolean;
  /** 结果说明(ok / no-path / timeout / error)。 */
  reason: string;
};

const FONT_RESULT_FALLBACK: FontResolve = {
  font: DEFAULT_FONT,
  applied: false,
  reason: "timeout",
};

/**
 * 读取设置中的终端字体文件路径并加载为自定义字体。
 * 未设置路径时返回默认字体;文件读取/解析失败时回退默认字体。
 */
async function resolveTerminalFont(): Promise<FontResolve> {
  try {
    const s = await invoke<Settings>("get_settings");
    const path = s.terminal_font_path?.trim();
    if (!path) {
      console.log("[TerminalFont] 未设置字体路径,使用默认字体");
      return { font: DEFAULT_FONT, applied: false, reason: "no-path" };
    }

    console.log("[TerminalFont] 开始加载字体文件:", path);
    // 通过 Tauri 内置 asset 协议加载本地字体文件(自带 CORS 处理)
    const face = new FontFace(CUSTOM_FONT_FAMILY, `url(${convertFileSrc(path)})`);
    const loaded = await withTimeout(face.load(), FONT_LOAD_TIMEOUT, null);
    if (!loaded) {
      console.warn("[TerminalFont] 字体加载超时或失败,回退默认字体:", path);
      return { font: DEFAULT_FONT, applied: false, reason: "timeout" };
    }
    document.fonts.add(loaded);
    console.log("[TerminalFont] 字体加载成功:", path);
    return { font: `"${CUSTOM_FONT_FAMILY}", ${DEFAULT_FONT}`, applied: true, reason: "ok" };
  } catch (e) {
    console.warn("[TerminalFont] 读取设置失败,使用默认字体:", e);
    return { font: DEFAULT_FONT, applied: false, reason: "error" };
  }
}

/** 应用字体到终端,并在字体变化后重新 fit、同步 PTY 尺寸。 */
function applyTerminalFont(
  term: Terminal,
  fit: FitAddon,
  font: string,
  doResize: () => void,
  result: FontResolve,
): void {
  if (term.options.fontFamily !== font) {
    term.options.fontFamily = font;
    console.log(
      `[TerminalFont] 终端字体已应用: ${result.applied ? "自定义字体" : "默认字体"} (${result.reason})`,
    );
    try {
      fit.fit();
      doResize();
    } catch {
      /* 容器隐藏时忽略 */
    }
  } else {
    console.log(
      `[TerminalFont] 字体未变化,保持当前字体 (${result.reason})`,
    );
  }
}

export const TerminalView = ({
  active,
  onSessionId,
}: {
  active: boolean;
  onSessionId: (sessionId: number) => void;
}) => {
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const sessionIdRef = useRef<number | null>(null);
  const onSessionIdRef = useRef(onSessionId);
  useEffect(() => {
    onSessionIdRef.current = onSessionId;
  }, [onSessionId]);

  useEffect(() => {
    const term = new Terminal({
      allowProposedApi: true,
      cursorBlink: true,
      fontSize: 14,
      // 字体从设置读取,设置变更时通过 settings-changed 事件实时更新
      fontFamily: DEFAULT_FONT,
      theme: { background: "#0c0c0c", foreground: "#cccccc" },
      scrollback: 5000,
      // 右键先选中光标处的单词(已有选区则保留),配合下方 contextmenu 处理实现"右键即复制"
      rightClickSelectsWord: true,
    });
    termRef.current = term;
    const fit = new FitAddon();
    fitRef.current = fit;
    term.loadAddon(fit);
    // Unicode 11 宽度检测:正确识别 emoji (U+1F000+) 为 2 格宽,
    // 修复默认 UnicodeV6 对 BMP 外字符一律返回 1 导致的行末溢出。
    const unicode11 = new Unicode11Addon();
    term.loadAddon(unicode11);
    term.unicode.activeVersion = "11";
    term.open(containerRef.current!);

    // 右键复制:阻止原生右键菜单,把选区(无选区时右击选中的单词)写入剪贴板。
    // xterm 的 rightClickSelectsWord 已把选区文本填入内部 textarea 并聚焦选中,
    // 这里在此基础上用 execCommand 同步复制(WebView2 可靠),失败时回退 Clipboard API。
    const onTermContextMenu = (e: MouseEvent) => {
      e.preventDefault();
      const text = term.getSelection();
      if (!text) return;
      try {
        const ta = term.textarea;
        if (ta) {
          ta.value = text;
          ta.focus();
          ta.select();
          if (document.execCommand("copy")) {
            // 与 PowerShell 一致:右键复制完成后清除选区高亮
            term.clearSelection();
            return;
          }
        }
      } catch {
        /* execCommand 失败时走 Clipboard API */
      }
      navigator.clipboard.writeText(text).then(
        () => term.clearSelection(),
        () => {},
      );
    };
    term.element?.addEventListener("contextmenu", onTermContextMenu);

    // WebGL 渲染器:字形按网格坐标绘制,不受 DOM 渲染器 letter-spacing 的
    // 边界漂移影响(Chromium 对含 CJK 的行会把右边界逐格推偏,导致右边框
    // 上下不齐,见 xtermjs/xterm.js#6058)。激活失败/上下文丢失时回退默认
    // DOM 渲染器。
    let webglAddon: WebglAddon | null = null;
    try {
      webglAddon = new WebglAddon();
      term.loadAddon(webglAddon);
      webglAddon.onContextLoss(() => {
        // 上下文无法恢复时卸载,自动恢复默认 DOM 渲染器
        webglAddon?.dispose();
      });
    } catch (e) {
      console.warn("[TerminalRenderer] WebGL 不可用,回退 DOM 渲染器:", e);
    }

    let disposed = false;
    const sessionIdRef: { current: number | null } = { current: null };
    const unlistenFns: (() => void)[] = [];

    const doResize = () => {
      const sid = sessionIdRef.current;
      if (sid != null && term.cols > 0 && term.rows > 0) {
        invoke("resize_terminal", { id: sid, cols: term.cols, rows: term.rows }).catch(() => {});
      }
    };

    // 应用设置中的字体(异步读取,先于/晚于终端创建都安全)
    withTimeout(resolveTerminalFont(), FONT_LOAD_TIMEOUT, FONT_RESULT_FALLBACK).then((result) => {
      if (!disposed) applyTerminalFont(term, fit, result.font, doResize, result);
    });

    // 先挂监听再创建会话,避免错过启动输出
    (async () => {
      try {
        const unOut = await listen<TerminalOutput>("terminal-output", (e) => {
          if (e.payload.id === sessionIdRef.current) term.write(e.payload.data);
        });
        if (disposed) {
          unOut();
          return;
        }
        unlistenFns.push(unOut);

        const unExit = await listen<TerminalExit>("terminal-exit", (e) => {
          if (e.payload.id === sessionIdRef.current) {
            term.write("\r\n\x1b[90m[进程已退出]\x1b[0m");
          }
        });
        if (disposed) {
          unExit();
          return;
        }
        unlistenFns.push(unExit);
      } catch {
        /* 忽略监听失败 */
      }

      try {
        // fit 紧贴 create_terminal,缩小窗口期:
        // 期间 ResizeObserver / 字体加载等事件循环任务不会导致尺寸过期。
        try {
          fit.fit();
        } catch {
          /* 容器尚未有尺寸,等激活时再 fit */
        }
        const cols = term.cols || 80;
        const rows = term.rows || 24;
        const id = await invoke<number>("create_terminal", { cols, rows });
        if (disposed) {
          invoke("kill_terminal", { id }).catch(() => {});
          return;
        }
        sessionIdRef.current = id;
        onSessionIdRef.current(id);
      } catch (e) {
        term.write(`\r\n\x1b[91m[启动终端失败] ${String(e)}\x1b[0m`);
      }
    })();

    const dataSub = term.onData((data) => {
      const sid = sessionIdRef.current;
      if (sid != null) {
        invoke("write_terminal", { id: sid, data }).catch(() => {});
      }
    });

    const ro = new ResizeObserver(() => {
      try {
        fit.fit();
        doResize();
      } catch {
        /* 容器隐藏时尺寸为 0 */
      }
    });
    if (containerRef.current) ro.observe(containerRef.current);

    // 设置保存后实时应用新字体
    let unSettingsChanged: (() => void) | undefined;
    listen("settings-changed", () => {}).then((un) => {
      if (disposed) {
        un();
        return;
      }
      unSettingsChanged = un;
      withTimeout(resolveTerminalFont(), FONT_LOAD_TIMEOUT, FONT_RESULT_FALLBACK).then((result) => {
        if (!disposed) applyTerminalFont(term, fit, result.font, doResize, result);
      });
    });

    return () => {
      disposed = true;
      const sid = sessionIdRef.current;
      if (sid != null) {
        invoke("kill_terminal", { id: sid }).catch(() => {});
      }
      dataSub.dispose();
      ro.disconnect();
      unlistenFns.forEach((f) => f());
      unSettingsChanged?.();
      term.element?.removeEventListener("contextmenu", onTermContextMenu);
      term.dispose();
    };
  }, []);

  // 切换为激活标签页时重新适配尺寸
  useEffect(() => {
    if (!active) return;
    const raf = requestAnimationFrame(() => {
      try {
        fitRef.current?.fit();
        const sid = sessionIdRef.current;
        const term = termRef.current;
        if (sid != null && term && term.cols > 0 && term.rows > 0) {
          invoke("resize_terminal", { id: sid, cols: term.cols, rows: term.rows }).catch(
            () => {},
          );
        }
      } catch {
        /* ignore */
      }
    });
    return () => cancelAnimationFrame(raf);
  }, [active]);

  return <div ref={containerRef} className="terminal-container" />;
};
