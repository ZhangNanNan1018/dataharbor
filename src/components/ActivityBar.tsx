import { Database, Info, Moon, Search, Sun } from "lucide-react";
import { useUiStore } from "../stores/useUiStore";

export function ActivityBar() {
  const theme = useUiStore((state) => state.theme);
  const toggleTheme = useUiStore((state) => state.toggleTheme);
  const collapsed = useUiStore((state) => state.sidebarCollapsed);
  const toggleSidebar = useUiStore((state) => state.toggleSidebar);
  const openPalette = useUiStore((state) => state.openPalette);
  const setAboutOpen = useUiStore((state) => state.setAboutOpen);

  return (
    <nav className="xb-activity" aria-label="活动栏">
      <button type="button" className={collapsed ? "" : "active"} onClick={toggleSidebar} title="连接与对象" aria-label="连接与对象">
        <Database size={18} />
      </button>
      <button type="button" onClick={() => openPalette("go")} title="快速打开 (Ctrl+P)" aria-label="快速打开">
        <Search size={18} />
      </button>
      <div className="xb-activity-spacer" />
      <button type="button" onClick={toggleTheme} title={theme === "dark" ? "浅色" : "深色"} aria-label="切换主题">
        {theme === "dark" ? <Sun size={18} /> : <Moon size={18} />}
      </button>
      <button type="button" onClick={() => setAboutOpen(true)} title="关于" aria-label="关于">
        <Info size={18} />
      </button>
    </nav>
  );
}
