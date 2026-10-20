import { Alert } from "antd";
import { useEffect } from "react";
import { api, isTauri } from "../api";
import { useConnectionStore } from "../stores/useConnectionStore";
import { useUiStore } from "../stores/useUiStore";
import { AboutDialog } from "./AboutDialog";
import { ActivityBar } from "./ActivityBar";
import { CommandPalette } from "./CommandPalette";
import { ConnectionDialog } from "./ConnectionDialog";
import { Sidebar } from "./Sidebar";
import { StatusBar } from "./StatusBar";
import { Toolbar } from "./Toolbar";
import { Workspace } from "./Workspace";

export function Shell() {
  const load = useConnectionStore((state) => state.load);
  const error = useConnectionStore((state) => state.error);
  const sidebarWidth = useUiStore((state) => state.sidebarWidth);
  const setSidebarWidth = useUiStore((state) => state.setSidebarWidth);
  const focusMode = useUiStore((state) => state.focusMode);
  const sidebarCollapsed = useUiStore((state) => state.sidebarCollapsed);
  const setStatus = useUiStore((state) => state.setStatus);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    if (!isTauri()) return;
    const timer = window.setInterval(() => {
      void api
        .runDueJobs()
        .then((messages) => {
          if (messages.length) setStatus(messages[messages.length - 1]);
        })
        .catch(() => undefined);
    }, 15000);
    return () => window.clearInterval(timer);
  }, [setStatus]);

  return (
    <div className="xb-shell flex h-full min-h-0 flex-col">
      <Toolbar />
      {!isTauri() && (
        <div className="xb-banner">
          <Alert
            type="warning"
            showIcon
            message="浏览器里只能看界面。连接、查询和改数据要在桌面窗口中进行：npm run tauri dev"
          />
        </div>
      )}
      {error && (
        <div className="xb-banner">
          <Alert type="error" showIcon message={error} />
        </div>
      )}
      <div className="xb-body">
        {!focusMode && <ActivityBar />}
        {!focusMode && !sidebarCollapsed && <Sidebar width={sidebarWidth} />}
        {!focusMode && !sidebarCollapsed && (
          <div
            className="xb-resize"
            onMouseDown={(event) => {
              event.preventDefault();
              const startX = event.clientX;
              const startW = sidebarWidth;
              const move = (ev: MouseEvent) => {
                setSidebarWidth(Math.min(480, Math.max(220, startW + ev.clientX - startX)));
              };
              const up = () => {
                window.removeEventListener("mousemove", move);
                window.removeEventListener("mouseup", up);
              };
              window.addEventListener("mousemove", move);
              window.addEventListener("mouseup", up);
            }}
          />
        )}
        <Workspace />
      </div>
      <StatusBar />
      <ConnectionDialog />
      <AboutDialog />
      <CommandPalette />
    </div>
  );
}
