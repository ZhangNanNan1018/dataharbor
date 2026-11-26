import { Database, Info, Moon, Search, Sun } from "lucide-react";
import { Dropdown } from "antd";
import { themeOptions, useUiStore } from "../stores/useUiStore";

export function ActivityBar() {
  const theme = useUiStore((state) => state.theme);
  const preference = useUiStore((state) => state.themePreference);
  const setTheme = useUiStore((state) => state.setTheme);
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
      <Dropdown trigger={["click"]} placement="topLeft" menu={{
        selectable: true, selectedKeys: [preference],
        items: themeOptions.map(option => ({ ...option, onClick: () => setTheme(option.key) })),
      }}>
        <button type="button" title="主题" aria-label="切换主题" aria-haspopup="menu">
          {theme === "dark" ? <Moon size={18} /> : <Sun size={18} />}
        </button>
      </Dropdown>
      <button type="button" onClick={() => setAboutOpen(true)} title="关于" aria-label="关于">
        <Info size={18} />
      </button>
    </nav>
  );
}
