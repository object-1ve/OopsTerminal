import { useEffect, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import "./SettingsModal.css";

type Settings = {
  default_path: string | null;
  show_tray_icon: boolean;
  show_taskbar_icon: boolean;
  terminal_font_path: string | null;
};

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
    // 先由后端解析 junction/符号链接(如 Scoop 的 current 目录),避免路径
    // 经过不受信任装入点时 asset 协议读不到文件导致加载超时。
    const realPath = await invoke<string>("resolve_terminal_font_path", { path });
    const face = new FontFace(CUSTOM_FONT_FAMILY, `url(${convertFileSrc(realPath)})`);
    const loaded = await withTimeout(face.load(), FONT_LOAD_TIMEOUT, null);
    if (!loaded) return "字体加载超时,文件可能不存在或格式不受支持";
    document.fonts.add(loaded);
    return "";
  } catch (e) {
    return `字体加载失败: ${String(e)}`;
  }
}

export const SettingsModal = ({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) => {
  const [defaultPath, setDefaultPath] = useState("");
  const [showTrayIcon, setShowTrayIcon] = useState(true);
  const [showTaskbarIcon, setShowTaskbarIcon] = useState(false);
  const [terminalFontPath, setTerminalFontPath] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [fontMessage, setFontMessage] = useState<{ ok: boolean; msg: string } | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (!open) return;
    invoke<Settings>("get_settings")
      .then((s) => {
        setError(null);
        setFontMessage(null);
        setDefaultPath(s.default_path ?? "");
        setShowTrayIcon(s.show_tray_icon);
        setShowTaskbarIcon(s.show_taskbar_icon);
        setTerminalFontPath(s.terminal_font_path ?? "");
      })
      .catch(() => setError("无法读取设置"));
  }, [open]);

  const save = async () => {
    setSaving(true);
    setError(null);
    setFontMessage(null);
    try {
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
    <div className="settings-overlay" onMouseDown={onClose}>
      <div className="settings-panel" onMouseDown={(e) => e.stopPropagation()}>
        <h2 className="settings-title">设置</h2>

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
          默认启动路径留空表示使用用户主目录。终端字体文件支持 ttf/otf/woff/woff2，留空使用默认字体。若通过浏览选择 Scoop 等目录中的字体提示“无法访问”（不受信任的装入点），可直接手动输入完整路径，应用会自动解析 current 链接到真实目录；系统已安装的字体也可在 C:\Users\用户名\AppData\Local\Microsoft\Windows\Fonts 中选择。
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
