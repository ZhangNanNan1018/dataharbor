import { App, Button, Dropdown } from "antd";
import type { MenuProps } from "antd";
import { PanelLeft, Search } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { errorText, isTauri } from "../api";
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

const FILE_COMMANDS = [
  { key: "query", label: "新建查询" },
  { key: "sample", label: "示例 SQLite" },
  { key: "refresh", label: "刷新连接" },
] as const;

let menuGeneration = 0;
let menuQueue: Promise<void> = Promise.resolve();
let activeMenu: { close: () => Promise<void> } | null = null;

function enqueueMenu(task: () => Promise<void>) {
  const run = menuQueue.then(task, task);
  menuQueue = run.then(
    () => undefined,
    () => undefined,
  );
  return run;
}

function focusLabel(focusMode: boolean) {
  return focusMode ? "退出专注" : "专注";
}

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
  const [nativeMenu, setNativeMenu] = useState(isTauri);
  const mod = typeof navigator !== "undefined" && /Mac/i.test(navigator.platform) ? "⌘" : "Ctrl";
  const runRef = useRef<(key: string) => void>(() => undefined);

  runRef.current = (key: string) => {
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
    openPlaceholder(key);
  };

  useEffect(() => {
    if (!isTauri()) return;
    let alive = true;
    const ticket = ++menuGeneration;
    const label = focusLabel(focusMode);
    void enqueueMenu(async () => {
      if (ticket !== menuGeneration) return;
      try {
        const { Menu, Submenu } = await import("@tauri-apps/api/menu");
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        const command = (id: string, text: string) => ({
          id,
          text,
          action: () => runRef.current(id),
        });
        const file = await Submenu.new({
          text: "文件",
          items: [
            ...FILE_COMMANDS.map((entry) => command(entry.key, entry.label)),
            command("focus", label),
          ],
        });
        const groups = await Promise.all(
          GROUPS.map((group) =>
            Submenu.new({
              text: group.label,
              items: group.ids.map((id) => command(id, PLACEHOLDERS[id]?.title ?? id)),
            }),
          ),
        );
        const menu = await Menu.new({ items: [file, ...groups] });
        if (ticket !== menuGeneration) {
          await menu.close();
          return;
        }
        const mac = /Mac/i.test(navigator.platform);
        const previous = mac ? await menu.setAsAppMenu() : await menu.setAsWindowMenu(getCurrentWindow());
        if (ticket !== menuGeneration) {
          await menu.close();
          return;
        }
        if (alive) setNativeMenu(true);
        activeMenu = menu;
        if (previous && previous !== activeMenu) await previous.close().catch(() => undefined);
      } catch {
        if (alive && ticket === menuGeneration) setNativeMenu(false);
      }
    });
    return () => {
      alive = false;
    };
  }, [focusMode]);

  const browserMenu: MenuProps["items"] = [
    {
      key: "file",
      label: "文件",
      children: [
        ...FILE_COMMANDS.map((entry) => ({ key: entry.key, label: entry.label })),
        { key: "focus", label: focusLabel(focusMode) },
      ],
    },
    ...GROUPS.map((group) => ({
      key: group.label,
      label: group.label,
      children: group.ids.map((id) => ({ key: id, label: PLACEHOLDERS[id]?.title ?? id })),
    })),
  ];

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
      {!nativeMenu && (
        <Dropdown
          menu={{
            items: browserMenu,
            onClick: ({ key }) => runRef.current(String(key)),
          }}
        >
          <Button size="small">工具</Button>
        </Dropdown>
      )}
    </header>
  );
}
