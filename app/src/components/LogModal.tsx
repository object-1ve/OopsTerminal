import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./LogModal.css";

type LogEntry = {
  time: string;
  id: number;
  content: string;
  cwd?: string;
};

type InputLogData = {
  path: string;
  entries: LogEntry[];
};

/** 把 ISO 时间转为 YYYY-MM-DD HH:MM:SS 显示格式。 */
function formatTime(iso: string): string {
  return new Date(iso).toLocaleString("sv-SE");
}
function renderContent(content: string): string {
  return Array.from(
    content.replace(/\r\n/g, "\n").replace(/\r/g, "\n").replace(/\t/g, "→    "),
  )
    .map((c) => {
      const code = c.charCodeAt(0);
      if (c === "\n") return "⏎\n";
      if (code === 0x7f || code === 0x08) return "⌫";
      if (code < 0x20) return `U+${code.toString(16).toUpperCase()}`;
      return c;
    })
    .join("");
}

export const LogModal = ({ open, onClose }: { open: boolean; onClose: () => void }) => {
  const [data, setData] = useState<InputLogData | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const load = () => {
    if (!open) return;
    setLoading(true);
    setError(null);
    invoke<InputLogData>("read_input_log")
      .then((d) => setData(d))
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    if (!open) return;
    // 延迟到下一帧再加载,避免在 effect 体内同步触发 setState
    const timer = setTimeout(load, 0);
    return () => clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  if (!open) return null;

  return (
    <div className="log-overlay" onMouseDown={onClose}>
      <div className="log-panel" onMouseDown={(e) => e.stopPropagation()}>
        <div className="log-header">
          <h2 className="log-title">输入记录</h2>
          <button type="button" className="log-refresh" onClick={load} disabled={loading}>
            {loading ? "刷新中…" : "刷新"}
          </button>
        </div>

        {data && (
          <div className="log-path" title={data.path}>
            日志文件: {data.path}
          </div>
        )}

        {error && <div className="log-error">{error}</div>}

        {data && data.entries.length === 0 && !error && (
          <div className="log-empty">暂无输入记录</div>
        )}

        <div className="log-list">
          {data?.entries.map((entry, i) => (
            <div className="log-entry" key={`${entry.time}-${entry.id}-${i}`}>
              <div className="log-entry-meta">
                {entry.cwd && <span className="log-entry-cwd">{entry.cwd}</span>}
                <span className="log-entry-time">{formatTime(entry.time)}</span>
                <span className="log-entry-id">会话 {entry.id}</span>
              </div>
              <pre className="log-entry-content">{renderContent(entry.content)}</pre>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
};
