import { App, Button, Dropdown } from "antd";
import { PanelLeft, Search } from "lucide-react";
import { errorText } from "../api";
import { openSampleBookstore } from "../actions";
import { PLACEHOLDERS } from "../placeholders";
import { useConnectionStore } from "../stores/useConnectionStore";
import { useTabStore } from "../stores/useTabStore";
import { useUiStore } from "../stores/useUiStore";
import { Logo } from "./Logo";

const GROUPS: { label: string; ids: string[] }[] = [
  { label: "查询", ids: ["builder", "search", "mongo", "debug"] },
  { label: "结构", ids: ["designer", "er", "schema-sync", "model"] },
  { label: "数据", ids: ["data-sync", "io", "backup", "generate", "redis"] },
  { label: "分析", ids: ["bi", "ai"] },
  { label: "运维", ids: ["jobs", "users", "collab"] },
];

export function Toolbar() {
  const { message } = App.useApp();
  const openConnection = useUiStore((state) => state.openConnection);
  const openPalette = useUiStore((state) => state.openPalette);
  const toggleSidebar = useUiStore((state) => state.toggleSidebar);
  const focusMode = useUiStore((state) => state.focusMode);
  const toggleFocus = useUiStore((state) => state.toggleFocus);
  const profiles = useConnectionStore((state) => state.profiles);
  const load = useConnectionStore((state) => state.load);
  const setStatus = useUiStore((state) => state.setStatus);
  const openQuery = useTabStore((state) => state.openQuery);
  const openPlaceholder = useTabStore((state) => state.openPlaceholder);
  const mod = typeof navigator !== "undefined" && /Mac/i.test(navigator.platform) ? "⌘" : "Ctrl";

  return (
    <header className="xb-toolbar">
      <button type="button" className="xb-icon-btn" onClick={toggleSidebar} aria-label="折叠侧栏">
        <PanelLeft size={16} />
      </button>
      <div className="xb-brand" data-tauri-drag-region>
        <Logo />
        <strong>DataHarbor</strong>
      </div>
      <button type="button" className="xb-palette-trigger" onClick={() => openPalette("command")}>
        <Search size={14} />
        <span>搜索命令、表或查询</span>
        <kbd>{mod} K</kbd>
      </button>
      <div className="flex-1" data-tauri-drag-region />
      <Button size="small" type="primary" onClick={() => openConnection(null)}>
        新建连接
      </Button>
      <Dropdown
        menu={{
          items: [
            { key: "query", label: "新建查询" },
            { key: "sample", label: "示例 SQLite" },
            { key: "refresh", label: "刷新连接" },
            { key: "focus", label: focusMode ? "退出专注" : "专注" },
            { type: "divider" },
            ...GROUPS.map((group) => ({
              type: "group" as const,
              label: group.label,
              children: group.ids.map((id) => ({ key: id, label: PLACEHOLDERS[id]?.title ?? id })),
            })),
          ],
          onClick: ({ key }) => {
            if (key === "query") {
              const current = profiles.find((item) => item.connected) ?? profiles[0];
              openQuery({
                connectionId: current?.id ?? null,
                database: current?.engine === "sqlite" ? "main" : current?.database || "",
                title: "查询",
              });
              return;
            }
            if (key === "sample") {
              void openSampleBookstore()
                .then(() => message.success("示例书店已就绪"))
                .catch((err) => message.error(errorText(err)));
              return;
            }
            if (key === "refresh") {
              void load();
              setStatus("已刷新连接列表");
              return;
            }
            if (key === "focus") {
              toggleFocus();
              return;
            }
            openPlaceholder(String(key));
          },
        }}
      >
        <Button size="small">工具</Button>
      </Dropdown>
    </header>
  );
}
