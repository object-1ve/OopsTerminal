import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./TitleBar.css";

const appWindow = getCurrentWindow();

export const TitleBar = () => {
  const [isMaximized, setIsMaximized] = useState(false);

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
        OopsAssistant
      </div>
      <div className="titlebar-button" id="titlebar-minimize" onClick={() => appWindow.minimize()}>
        <svg width="10" height="1" viewBox="0 0 10 1" fill="none" xmlns="http://www.w3.org/2000/svg">
          <rect width="10" height="1" />
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
      <div className="titlebar-button" id="titlebar-close" onClick={() => appWindow.close()}>
        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" xmlns="http://www.w3.org/2000/svg">
          <path d="M1 1L9 9" strokeWidth="1" />
          <path d="M9 1L1 9" strokeWidth="1" />
        </svg>
      </div>
    </div>
  );
};
