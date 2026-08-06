import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./TitleBar.css";

const appWindow = getCurrentWindow();

export const TitleBar = ({
  onOpenSettings,
  onOpenLog,
}: {
  onOpenSettings: () => void;
  onOpenLog: () => void;
}) => {
  const [isMaximized, setIsMaximized] = useState(false);
  const [isPinned, setIsPinned] = useState(false);

  useEffect(() => {
    appWindow.isAlwaysOnTop().then(setIsPinned).catch(() => {});
  }, []);

  const togglePin = async () => {
    const next = !isPinned;
    setIsPinned(next);
    try {
      await appWindow.setAlwaysOnTop(next);
    } catch {
      setIsPinned(!next);
    }
  };

  useEffect(() => {
    const updateMaximized = async () => {
      setIsMaximized(await appWindow.isMaximized());
    };

    updateMaximized();

    const unlisten = appWindow.onResized(() => {
      updateMaximized();
    });

    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  return (
    <div className="titlebar" data-tauri-drag-region>
      <div className="titlebar-drag-region" data-tauri-drag-region>
        OopsTerminal
      </div>
      <div className="titlebar-button" id="titlebar-log" onClick={onOpenLog} title="输入记录">
        <svg width="10" height="10" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
          <path d="M4 3h12l4 4v14H4V3zm4 4h6v2H8V7zm0 4h10v2H8v-2zm0 4h10v2H8v-2zM14.5 3.5V7H18l-3.5-3.5z" />
        </svg>
      </div>
      <div
        className={`titlebar-button ${isPinned ? "active" : ""}`}
        id="titlebar-pin"
        onClick={togglePin}
        title={isPinned ? "取消置顶" : "置顶"}
      >
        <svg width="10" height="10" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
          <path d="M16 3l5 5-4.5 1.5L13 13l1 6-2 2-4-4-4 4-2-2 4-4-4-4 2-2 6 1 3.5-3.5L16 3z" />
        </svg>
      </div>
      <div className="titlebar-button" id="titlebar-settings" onClick={onOpenSettings} title="设置">
        <svg width="10" height="10" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
          <circle cx="12" cy="12" r="3" />
          <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09a1.65 1.65 0 0 0-1-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09a1.65 1.65 0 0 0 1.51-1 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33h.01a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51h.01a1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82v.01a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
        </svg>
      </div>
      <div className="titlebar-button" id="titlebar-maximize" onClick={() => appWindow.toggleMaximize()}>
        {isMaximized ? (
          <svg width="10" height="10" viewBox="0 0 10 10" xmlns="http://www.w3.org/2000/svg">
            <path d="M2.1,0v2H0v8.1h8.2v-2h2.1V0H2.1z M7.2,9.2H1.1V3.1h6.1V9.2z M9.2,7.1H8.2V3.1c0-0.6-0.5-1.1-1.1-1.1H3.1V1.1h6.1V7.1z" />
          </svg>
        ) : (
          <svg width="10" height="10" viewBox="0 0 10 10" xmlns="http://www.w3.org/2000/svg">
            <path d="M0,0v10h10V0H0z M9,9H1V1h8V9z" />
          </svg>
        )}
      </div>
      <div className="titlebar-button" id="titlebar-minimize" onClick={() => appWindow.minimize()}>
        <svg width="10" height="1" viewBox="0 0 10 1" fill="none" xmlns="http://www.w3.org/2000/svg">
          <rect width="10" height="1" />
        </svg>
      </div>
    </div>
  );
};
