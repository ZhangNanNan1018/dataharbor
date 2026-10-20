import { useConnectionStore } from "../stores/useConnectionStore";
import { useTabStore } from "../stores/useTabStore";
import { useUiStore } from "../stores/useUiStore";

export function StatusBar() {
  const status = useUiStore((state) => state.status);
  const theme = useUiStore((state) => state.theme);
  const profiles = useConnectionStore((state) => state.profiles);
  const info = useConnectionStore((state) => state.info);
  const tabs = useTabStore((state) => state.tabs);
  const pinned = useUiStore((state) => state.pinned.length);
  const connected = profiles.filter((item) => item.connected).length;

  return (
    <footer className="xb-status">
      <span>
        已连接 {connected}/{profiles.length}
        {info ? ` · 凭据：${info.encryption}` : ""}
      </span>
      <span className="min-w-0 flex-1 truncate text-center">{status}</span>
      <span>
        {tabs.length} 个标签 · 固定 {pinned} · {theme === "dark" ? "深色" : "浅色"} · {info?.version ?? "0.1.0"}
      </span>
    </footer>
  );
}
