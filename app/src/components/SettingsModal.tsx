import { useEffect, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import "./SettingsModal.css";

type Settings = {
  toggle_window_shortcut: string | null;
  quit_shortcut: string | null;
  default_path: string | null;
  show_tray_icon: boolean;
  show_taskbar_icon: boolean;
  terminal_font_path: string | null;
};

type ShortcutKey = "toggle_window_shortcut" | "quit_shortcut";

const MODIFIER_ONLY = new Set(["Control", "Alt", "Shift", "Meta"]);
const CUSTOM_FONT_FAMILY = "OopsTerminalCustomFont";
const FONT_LOAD_TIMEOUT = 8000;

/** 给 Promise 加超时。 */
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

/** 尝试加载字体文件,返回错误信息(空字符串表示成功)。 */
async function verifyFontFile(path: string): Promise<string> {
  try {
    const face = new FontFace(CUSTOM_FONT_FAMILY, `url(${convertFileSrc(path)})`);
    const loaded = await withTimeout(face.load(), FONT_LOAD_TIMEOUT, null);
    if (!loaded) return "字体加载超时,文件可能不存在或格式不受支持";
    document.fonts.add(loaded);
    return "";
  } catch (e) {
    return `字体加载失败: ${String(e)}`;
  }
}

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
  const [defaultPath, setDefaultPath] = useState("");
  const [showTrayIcon, setShowTrayIcon] = useState(true);
  const [showTaskbarIcon, setShowTaskbarIcon] = useState(false);
  const [terminalFontPath, setTerminalFontPath] = useState("");
  const [recording, setRecording] = useState<ShortcutKey | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [fontMessage, setFontMessage] = useState<{ ok: boolean; msg: string } | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (!open) return;
    invoke<Settings>("get_settings")
      .then((s) => {
        setRecording(null);
        setError(null);
        setFontMessage(null);
        setToggleShortcut(s.toggle_window_shortcut);
        setQuitShortcut(s.quit_shortcut);
        setDefaultPath(s.default_path ?? "");
        setShowTrayIcon(s.show_tray_icon);
        setShowTaskbarIcon(s.show_taskbar_icon);
        setTerminalFontPath(s.terminal_font_path ?? "");
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
    setFontMessage(null);
    try {
      await invoke("set_shortcut", { kind: "toggle", accel: toggleShortcut });
      await invoke("set_shortcut", { kind: "quit", accel: quitShortcut });
      await invoke("set_default_path", {
        path: defaultPath.trim() || null,
      });
      await invoke("set_ui_settings", {
        showTrayIcon,
        showTaskbarIcon,
      });
      const fontPath = terminalFontPath.trim();
      await invoke("set_terminal_font_path", {
        path: fontPath || null,
      });

      // 验证字体文件能否加载,把结果反馈给用户
      if (fontPath) {
        const err = await verifyFontFile(fontPath);
        if (err) {
          console.warn("[TerminalFont] 设置界面验证失败:", err);
          setFontMessage({ ok: false, msg: `字体路径已保存,但${err},终端将使用默认字体` });
        } else {
          console.log("[TerminalFont] 设置界面验证成功:", fontPath);
          setFontMessage({ ok: true, msg: `字体已应用成功: ${fontPath}` });
        }
      } else {
        console.log("[TerminalFont] 已恢复默认字体");
        setFontMessage({ ok: true, msg: "已恢复默认字体" });
      }
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

        <div className="settings-row">
          <div className="settings-label">终端默认启动路径</div>
          <input
            type="text"
            className="settings-path-input"
            placeholder="例如 C:\Users\yourname"
            value={defaultPath}
            onChange={(e) => setDefaultPath(e.target.value)}
          />
          <button
            type="button"
            className="shortcut-clear"
            onClick={() => setDefaultPath("")}
          >
            清除
          </button>
        </div>

        <div className="settings-row">
          <div className="settings-label">终端字体文件</div>
          <input
            type="text"
            className="settings-path-input"
            placeholder="例如 C:\Fonts\SarasaMonoSC-Regular.ttf"
            value={terminalFontPath}
            onChange={(e) => setTerminalFontPath(e.target.value)}
          />
          <button
            type="button"
            className="shortcut-clear"
            onClick={async () => {
              const selected = await openDialog({
                multiple: false,
                filters: [
                  { name: "字体文件", extensions: ["ttf", "otf", "woff", "woff2"] },
                ],
              });
              if (typeof selected === "string") {
                setTerminalFontPath(selected);
              }
            }}
          >
            浏览…
          </button>
          <button
            type="button"
            className="shortcut-clear"
            onClick={() => setTerminalFontPath("")}
          >
            清除
          </button>
        </div>

        <div className="settings-row">
          <div className="settings-label">显示托盘图标</div>
          <label className="settings-switch">
            <input
              type="checkbox"
              checked={showTrayIcon}
              onChange={(e) => setShowTrayIcon(e.target.checked)}
            />
            <span className="settings-slider" />
          </label>
          <span className="settings-switch-value">{showTrayIcon ? "显示" : "隐藏"}</span>
        </div>

        <div className="settings-row">
          <div className="settings-label">显示任务栏图标</div>
          <label className="settings-switch">
            <input
              type="checkbox"
              checked={showTaskbarIcon}
              onChange={(e) => setShowTaskbarIcon(e.target.checked)}
            />
            <span className="settings-slider" />
          </label>
          <span className="settings-switch-value">
            {showTaskbarIcon ? "显示" : "隐藏"}
          </span>
        </div>

        <p className="settings-hint">
          点击输入框后按下组合键（如 Ctrl+Shift+K）。显示/隐藏快捷键可切换窗口显隐，留空表示禁用。默认启动路径留空表示使用用户主目录。终端字体文件支持 ttf/otf/woff/woff2，留空使用默认字体。
        </p>

        {error && <p className="settings-error">{error}</p>}
        {fontMessage && (
          <p className={fontMessage.ok ? "settings-message ok" : "settings-message err"}>
            {fontMessage.msg}
          </p>
        )}

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
