import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./SettingsModal.css";

type Settings = {
  toggle_window_shortcut: string | null;
  quit_shortcut: string | null;
};

type ShortcutKey = "toggle_window_shortcut" | "quit_shortcut";

const MODIFIER_ONLY = new Set(["Control", "Alt", "Shift", "Meta"]);

/** Build a tauri-compatible accelerator string, or null for unsupported keys. */
function buildAccelerator(e: KeyboardEvent): string | null {
  const parts: string[] = [];
  if (e.ctrlKey) parts.push("Ctrl");
  if (e.altKey) parts.push("Alt");
  if (e.shiftKey) parts.push("Shift");
  if (e.metaKey) parts.push("Super");

  const key = e.key;
  if (MODIFIER_ONLY.has(key)) return null;

  let main: string | null = null;
  if (/^[a-zA-Z]$/.test(key)) {
    main = key.toUpperCase();
  } else if (/^[0-9]$/.test(key)) {
    main = key;
  } else if (/^F([1-9]|1[0-9]|2[0-4])$/.test(key)) {
    main = key;
  }

  if (!main || parts.length === 0) return null;
  return [...parts, main].join("+");
}

export const SettingsModal = ({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) => {
  const [toggleShortcut, setToggleShortcut] = useState<string | null>(null);
  const [quitShortcut, setQuitShortcut] = useState<string | null>(null);
  const [recording, setRecording] = useState<ShortcutKey | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (!open) return;
    setRecording(null);
    setError(null);
    invoke<Settings>("get_settings")
      .then((s) => {
        setToggleShortcut(s.toggle_window_shortcut);
        setQuitShortcut(s.quit_shortcut);
      })
      .catch(() => setError("无法读取设置"));
  }, [open]);

  useEffect(() => {
    if (!open || !recording) return;

    const onKeyDown = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      const accel = buildAccelerator(e);
      if (!accel) return; // modifier-only or unsupported key: keep recording
      setRecording(null);
      if (recording === "toggle_window_shortcut") setToggleShortcut(accel);
      else setQuitShortcut(accel);
    };

    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [open, recording]);

  const save = async () => {
    setSaving(true);
    setError(null);
    try {
      await invoke("set_shortcut", { kind: "toggle", accel: toggleShortcut });
      await invoke("set_shortcut", { kind: "quit", accel: quitShortcut });
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  if (!open) return null;

  return (
    <div className="settings-overlay" onMouseDown={() => recording ? undefined : onClose()}>
      <div className="settings-panel" onMouseDown={(e) => e.stopPropagation()}>
        <h2 className="settings-title">设置</h2>

        <div className="settings-row">
          <div className="settings-label">显示/隐藏窗口快捷键</div>
          <button
            type="button"
            className={`shortcut-box ${recording === "toggle_window_shortcut" ? "recording" : ""}`}
            onClick={() => setRecording(recording === "toggle_window_shortcut" ? null : "toggle_window_shortcut")}
          >
            {recording === "toggle_window_shortcut"
              ? "请按下快捷键…"
              : toggleShortcut ?? "未设置"}
          </button>
          <button
            type="button"
            className="shortcut-clear"
            disabled={recording === "toggle_window_shortcut" || !toggleShortcut}
            onClick={() => setToggleShortcut(null)}
          >
            清除
          </button>
        </div>

        <div className="settings-row">
          <div className="settings-label">退出程序快捷键</div>
          <button
            type="button"
            className={`shortcut-box ${recording === "quit_shortcut" ? "recording" : ""}`}
            onClick={() => setRecording(recording === "quit_shortcut" ? null : "quit_shortcut")}
          >
            {recording === "quit_shortcut"
              ? "请按下快捷键…"
              : quitShortcut ?? "未设置"}
          </button>
          <button
            type="button"
            className="shortcut-clear"
            disabled={recording === "quit_shortcut" || !quitShortcut}
            onClick={() => setQuitShortcut(null)}
          >
            清除
          </button>
        </div>

        <p className="settings-hint">
          点击输入框后按下组合键（如 Ctrl+Shift+K）。显示/隐藏快捷键可切换窗口显隐，留空表示禁用。
        </p>

        {error && <p className="settings-error">{error}</p>}

        <div className="settings-actions">
          <button type="button" className="settings-btn" onClick={onClose}>
            取消
          </button>
          <button
            type="button"
            className="settings-btn primary"
            onClick={save}
            disabled={saving}
          >
            保存
          </button>
        </div>
      </div>
    </div>
  );
};
