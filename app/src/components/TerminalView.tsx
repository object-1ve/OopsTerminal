import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "@xterm/xterm/css/xterm.css";
import "./TerminalView.css";

type TerminalOutput = { id: number; data: string };
type TerminalExit = { id: number };
type Settings = { terminal_font_path: string | null };
type FontFile = { mime: string; data: string };

/** 默认终端字体。 */
const DEFAULT_FONT = 'Consolas, "Cascadia Mono", "Courier New", monospace';

/** 注册到页面里的自定义字体家族名。 */
const CUSTOM_FONT_FAMILY = "OopsTerminalCustomFont";

/**
 * 读取设置中的终端字体文件路径并加载为自定义字体。
 * 未设置路径时返回默认字体;文件读取/解析失败时回退默认字体。
 */
async function resolveTerminalFont(): Promise<string> {
  try {
    const s = await invoke<Settings>("get_settings");
    const path = s.terminal_font_path?.trim();
    if (!path) return DEFAULT_FONT;

    const font = await invoke<FontFile>("read_font_file", { path });
    if (!font.data) return DEFAULT_FONT;

    // 注册字体到文档,后续 term.options.fontFamily 引用该家族名即可
    const face = new FontFace(CUSTOM_FONT_FAMILY, `url(data:${font.mime};base64,${font.data})`);
    const loaded = await face.load();
    document.fonts.add(loaded);
    await document.fonts.ready;
    return `"${CUSTOM_FONT_FAMILY}", ${DEFAULT_FONT}`;
  } catch {
    return DEFAULT_FONT;
  }
}

/** 应用字体到终端,并在字体变化后重新 fit、同步 PTY 尺寸。 */
function applyTerminalFont(
  term: Terminal,
  fit: FitAddon,
  font: string,
  doResize: () => void,
): void {
  if (term.options.fontFamily !== font) {
    term.options.fontFamily = font;
    try {
      fit.fit();
      doResize();
    } catch {
      /* 容器隐藏时忽略 */
    }
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
    resolveTerminalFont().then((font) => {
      if (!disposed) applyTerminalFont(term, fit, font, doResize);
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
      resolveTerminalFont().then((font) => {
        if (!disposed) applyTerminalFont(term, fit, font, doResize);
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
