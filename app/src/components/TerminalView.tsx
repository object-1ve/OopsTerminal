import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "@xterm/xterm/css/xterm.css";
import "./TerminalView.css";

type TerminalOutput = { id: number; data: string };
type TerminalExit = { id: number };

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
      cursorBlink: true,
      fontSize: 14,
      // 使用系统默认等宽字体,不打包字体。
      fontFamily: 'Consolas, "Cascadia Mono", "Courier New", monospace',
      theme: { background: "#0c0c0c", foreground: "#cccccc" },
      scrollback: 5000,
    });
    termRef.current = term;
    const fit = new FitAddon();
    fitRef.current = fit;
    term.loadAddon(fit);
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

    return () => {
      disposed = true;
      const sid = sessionIdRef.current;
      if (sid != null) {
        invoke("kill_terminal", { id: sid }).catch(() => {});
      }
      dataSub.dispose();
      ro.disconnect();
      unlistenFns.forEach((f) => f());
      term.dispose();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
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
