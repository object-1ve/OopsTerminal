import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./HistoryModal.css";

type HistoryEntry = {
  time: string | null;
  command: string;
};

type OopsHistory = {
  source: "oops" | "psreadline" | "none";
  total: number;
  entries: HistoryEntry[];
};

const formatTime = (value: string | null) => {
  if (!value) return "";

  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;

  const pad = (part: number) => String(part).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(
    date.getDate(),
  )} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
};

export const HistoryModal = ({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) => {
  const [data, setData] = useState<OopsHistory | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const load = () => {
    if (!open) return;
    setLoading(true);
    setError(null);
    invoke<OopsHistory>("read_oops_history")
      .then(setData)
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    if (!open) return;
    setData(null);
    const timer = setTimeout(load, 0);
    return () => clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  if (!open) return null;

  const sourceLabel =
    data?.source === "oops"
      ? "OopsTerminal 带时间历史"
      : data?.source === "psreadline"
        ? "PSReadLine 历史"
        : "";

  const emptyMessage =
    data?.source === "none"
      ? "输入命令后会自动记录带时间历史"
      : data?.source === "oops"
        ? "暂无带时间的历史记录"
        : "暂无历史记录";

  return (
    <div className="history-overlay" onMouseDown={onClose}>
      <div className="history-panel" onMouseDown={(e) => e.stopPropagation()}>
        <div className="history-header">
          <h2 className="history-title">历史记录</h2>
          <button type="button" className="history-refresh" onClick={load} disabled={loading}>
            {loading ? "刷新中…" : "刷新"}
          </button>
        </div>

        {data && (
          <div className="history-count">
            {sourceLabel}
            {sourceLabel ? " · " : ""}
            共 {data.total} 条
            {data.total > data.entries.length
              ? `，显示最新 ${data.entries.length} 条`
              : ""}
          </div>
        )}

        {error && <div className="history-error">{error}</div>}

        {data && data.entries.length === 0 && !error && (
          <div className="history-empty">{emptyMessage}</div>
        )}

        <div className="history-list">
          {data?.entries.map((entry, i) => (
            <div
              className="history-entry"
              key={`${i}-${entry.time ?? "no-time"}-${entry.command}`}
            >
              <div className="history-entry-time">
                {formatTime(entry.time) || "无时间"}
              </div>
              <pre className="history-content">{entry.command}</pre>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
};
