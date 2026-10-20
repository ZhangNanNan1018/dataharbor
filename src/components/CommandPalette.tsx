import { useEffect, useMemo, useState } from "react";
import { api, errorText } from "../api";
import { openSampleBookstore } from "../actions";
import { engineLabel } from "../engines";
import { PLACEHOLDERS } from "../placeholders";
import { useConnectionStore } from "../stores/useConnectionStore";
import { useTabStore } from "../stores/useTabStore";
import { useUiStore } from "../stores/useUiStore";
import { App } from "antd";

interface Entry {
  id: string;
  group: string;
  label: string;
  hint?: string;
  run: () => void;
}

export function CommandPalette() {
  const { message } = App.useApp();
  const palette = useUiStore((state) => state.palette);
  const openPalette = useUiStore((state) => state.openPalette);
  const closePalette = useUiStore((state) => state.closePalette);
  const openConnection = useUiStore((state) => state.openConnection);
  const toggleTheme = useUiStore((state) => state.toggleTheme);
  const theme = useUiStore((state) => state.theme);
  const toggleFocus = useUiStore((state) => state.toggleFocus);
  const focusMode = useUiStore((state) => state.focusMode);
  const setAboutOpen = useUiStore((state) => state.setAboutOpen);
  const setStatus = useUiStore((state) => state.setStatus);
  const profiles = useConnectionStore((state) => state.profiles);
  const load = useConnectionStore((state) => state.load);
  const markConnected = useConnectionStore((state) => state.markConnected);
  const tabs = useTabStore((state) => state.tabs);
  const setActive = useTabStore((state) => state.setActive);
  const openQuery = useTabStore((state) => state.openQuery);
  const openData = useTabStore((state) => state.openData);
  const openPlaceholder = useTabStore((state) => state.openPlaceholder);
  const [query, setQuery] = useState("");
  const [cursor, setCursor] = useState(0);
  const [objects, setObjects] = useState<Entry[]>([]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const mod = event.metaKey || event.ctrlKey;
      if (!mod) return;
      const key = event.key.toLowerCase();
      if (key !== "k" && key !== "p") return;
      event.preventDefault();
      event.stopPropagation();
      openPalette(key === "p" ? "go" : "command");
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [openPalette]);

  useEffect(() => {
    if (!palette) return;
    setQuery("");
    setCursor(0);
  }, [palette]);

  useEffect(() => {
    if (palette !== "go") return;
    let cancelled = false;
    void (async () => {
      const found: Entry[] = [];
      const connected = profiles.filter((item) => item.connected);
      for (const profile of connected.slice(0, 6)) {
        const database = profile.engine === "sqlite" ? "main" : profile.database;
        const schema = profile.engine === "postgres" ? "public" : profile.engine === "sqlite" ? "main" : "";
        try {
          const [tables, views] = await Promise.all([
            api.listTables(profile.id, database, schema),
            api.listViews(profile.id, database, schema).catch(() => [] as string[]),
          ]);
          for (const name of tables) {
            found.push({
              id: `t-${profile.id}-${name}`,
              group: profile.name,
              label: name,
              hint: "表",
              run: () => openData({ connectionId: profile.id, database, schema, table: name, title: name, readOnly: false }),
            });
          }
          for (const name of views) {
            found.push({
              id: `v-${profile.id}-${name}`,
              group: profile.name,
              label: name,
              hint: "视图",
              run: () => openData({ connectionId: profile.id, database, schema, table: name, title: name, readOnly: true }),
            });
          }
        } catch {
          // A connection that cannot list objects still appears as itself.
        }
      }
      try {
        const favorites = await api.listFavorites();
        for (const favorite of favorites) {
          found.push({
            id: `fav-${favorite.id}`,
            group: "收藏",
            label: favorite.objectName,
            hint: favorite.kind,
            run: () =>
              openData({
                connectionId: favorite.connectionId,
                database: favorite.database,
                schema: favorite.schema,
                table: favorite.objectName,
                title: favorite.objectName,
                readOnly: favorite.kind === "view",
              }),
          });
        }
      } catch {
        // Favorites are optional in the browser preview.
      }
      if (!cancelled) setObjects(found);
    })();
    return () => {
      cancelled = true;
    };
  }, [palette, profiles, openData]);

  const commands = useMemo<Entry[]>(() => {
    const toolGroups: Record<string, string> = {
      builder: "查询",
      designer: "结构",
      er: "结构",
      "schema-sync": "结构",
      "data-sync": "数据",
      io: "数据",
      backup: "数据",
      generate: "数据",
      bi: "分析",
      model: "结构",
      jobs: "运维",
      users: "运维",
      ai: "分析",
      collab: "运维",
      redis: "数据",
      mongo: "查询",
      debug: "查询",
      search: "查询",
    };
    const items: Entry[] = [
      {
        id: "new-conn",
        group: "连接",
        label: "新建连接",
        run: () => openConnection(null),
      },
      {
        id: "sample",
        group: "连接",
        label: "打开示例书店",
        run: () => {
          void openSampleBookstore()
            .then(() => message.success("示例书店已就绪"))
            .catch((err) => message.error(errorText(err)));
        },
      },
      {
        id: "refresh",
        group: "连接",
        label: "刷新连接列表",
        run: () => {
          void load();
          setStatus("已刷新连接列表");
        },
      },
      {
        id: "query",
        group: "查询",
        label: "新建查询",
        hint: "Ctrl+Enter 运行",
        run: () => {
          const current = profiles.find((item) => item.connected) ?? profiles[0];
          openQuery({
            connectionId: current?.id ?? null,
            database: current?.engine === "sqlite" ? "main" : current?.database || "",
            title: "查询",
          });
        },
      },
      {
        id: "go",
        group: "查询",
        label: "快速打开表或视图",
        hint: "Ctrl+P",
        run: () => openPalette("go"),
      },
      {
        id: "focus",
        group: "工作区",
        label: focusMode ? "退出专注" : "进入专注",
        run: toggleFocus,
      },
      {
        id: "theme",
        group: "工作区",
        label: theme === "dark" ? "切换到浅色" : "切换到深色",
        run: toggleTheme,
      },
      {
        id: "about",
        group: "工作区",
        label: "关于 DataHarbor",
        run: () => setAboutOpen(true),
      },
    ];
    for (const spec of Object.values(PLACEHOLDERS)) {
      items.push({
        id: spec.id,
        group: toolGroups[spec.id] ?? "工具",
        label: spec.title,
        run: () => openPlaceholder(spec.id),
      });
    }
    for (const profile of profiles) {
      items.push({
        id: `conn-${profile.id}`,
        group: "连接",
        label: `编辑 ${profile.name}`,
        hint: engineLabel(profile.engine),
        run: () => openConnection(profile.id),
      });
    }
    return items;
  }, [
    focusMode,
    load,
    message,
    openConnection,
    openPalette,
    openPlaceholder,
    openQuery,
    profiles,
    setAboutOpen,
    setStatus,
    theme,
    toggleFocus,
    toggleTheme,
  ]);

  const goEntries = useMemo<Entry[]>(() => {
    const tabEntries: Entry[] = tabs.map((tab) => ({
      id: `tab-${tab.id}`,
      group: "已打开",
      label: tab.title,
      hint: "标签",
      run: () => setActive(tab.id),
    }));
    const connEntries: Entry[] = profiles.map((profile) => ({
      id: `open-${profile.id}`,
      group: "连接",
      label: profile.name,
      hint: profile.connected ? "已连接" : engineLabel(profile.engine),
      run: () => {
        void api
          .connect(profile.id)
          .then((result) => {
            markConnected(profile.id, true);
            setStatus(`已连接 ${profile.name} · ${result.version}`);
            const database = profile.engine === "sqlite" ? "main" : profile.database;
            openQuery({ connectionId: profile.id, database, title: `查询 · ${profile.name}` });
          })
          .catch((err) => message.error(errorText(err)));
      },
    }));
    return [...tabEntries, ...connEntries, ...objects];
  }, [markConnected, message, objects, openQuery, profiles, setActive, setStatus, tabs]);

  const source = palette === "go" ? goEntries : commands;
  const filtered = source.filter((item) => {
    const hay = `${item.group} ${item.label} ${item.hint ?? ""}`.toLowerCase();
    return hay.includes(query.trim().toLowerCase());
  });
  const current = filtered[Math.min(cursor, Math.max(filtered.length - 1, 0))];

  if (!palette) return null;

  return (
    <div className="xb-palette" onMouseDown={() => closePalette()}>
      <div
        className="xb-palette-card"
        onMouseDown={(event) => event.stopPropagation()}
        role="dialog"
        aria-label={palette === "go" ? "快速打开" : "命令"}
      >
        <input
          autoFocus
          placeholder={palette === "go" ? "打开表、视图、收藏或已打开的标签" : "搜索命令"}
          value={query}
          onChange={(event) => {
            setQuery(event.target.value);
            setCursor(0);
          }}
          onKeyDown={(event) => {
            if (event.key === "Escape") closePalette();
            if (event.key === "ArrowDown") {
              event.preventDefault();
              setCursor((value) => Math.min(filtered.length - 1, value + 1));
            }
            if (event.key === "ArrowUp") {
              event.preventDefault();
              setCursor((value) => Math.max(0, value - 1));
            }
            if (event.key === "Enter" && current) {
              closePalette();
              current.run();
            }
          }}
        />
        <div className="xb-palette-list">
          {filtered.length === 0 && <div className="xb-palette-group">没有匹配</div>}
          {filtered.map((item, index) => {
            const previous = filtered[index - 1];
            return (
              <div key={item.id}>
                {item.group !== previous?.group && <div className="xb-palette-group">{item.group}</div>}
                <button
                  type="button"
                  className={`xb-palette-item${index === cursor ? " active" : ""}`}
                  onMouseEnter={() => setCursor(index)}
                  onClick={() => {
                    closePalette();
                    item.run();
                  }}
                >
                  <span>{item.label}</span>
                  {item.hint && <small>{item.hint}</small>}
                </button>
              </div>
            );
          })}
        </div>
        <div className="xb-hint">↑↓ 选择 · Enter 执行 · Esc 关闭 · Ctrl/Cmd+K 命令 · Ctrl/Cmd+P 打开对象</div>
      </div>
    </div>
  );
}
