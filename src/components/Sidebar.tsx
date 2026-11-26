import { Database, Eye, Folder, Table2 } from "lucide-react";
import { Alert, App, Button, Empty, Input, Modal, Spin, Tree } from "antd";
import type { DataNode, EventDataNode } from "antd/es/tree";
import { useEffect, useMemo, useRef, useState } from "react";
import { api, errorText } from "../api";
import { engineLabel, qualifiedTable, usesSchema } from "../engines";
import { touchRecent } from "../recent";
import type { StudioContext } from "../types";
import { sessionProfile, useConnectionStore } from "../stores/useConnectionStore";
import { useTabStore } from "../stores/useTabStore";
import { useUiStore } from "../stores/useUiStore";
import type { ConnectionPublic, Engine } from "../types";
import { EngineGlyph } from "./EngineGlyph";
import { belongsToConnection } from "../catalogTree";
import { invalidateSqlCatalog } from "../sqlCatalog";
import { ensureConnection } from "../connectionReady";
import type { GroupEdit } from "../generated/contracts";

const SEP = "\u001f";

interface NodeMeta {
  kind: "group" | "connection" | "database" | "schema" | "folder" | "table" | "view" | "column" | "note";
  connectionId?: string;
  engine?: Engine;
  database?: string;
  schema?: string;
  name?: string;
  folder?: "tables" | "views";
  groupPath?: string;
  searchText: string;
}

type XNode = DataNode & { meta: NodeMeta; children?: XNode[] };

function nodeKey(...parts: string[]): string {
  return parts.join(SEP);
}

function groupParts(path: string) {
  return path.split("/").map((part) => part.trim()).filter(Boolean);
}

function inGroup(path: string, group: string) {
  return path === group || path.startsWith(group + "/");
}

function remapGroupKeys(keys: string[], source: string, edit: GroupEdit, hasDirect: boolean) {
  const prefix = nodeKey("group", "");
  const parent = source.split("/").slice(0, -1);
  const target = edit.kind === "rename" ? [...parent, edit.name.trim()] : parent;
  const mapped = keys.flatMap((key) => {
    const path = key.slice(prefix.length);
    if (!key.startsWith(prefix) || !inGroup(path, source)) return [key];
    if (path === source && edit.kind === "dissolve" && !hasDirect) return [];
    const suffix = path.slice(source.length).split("/").filter(Boolean);
    return [prefix + [...target, ...suffix].join("/")];
  });
  return [...new Set(mapped)];
}

export function Sidebar({ width }: { width: number }) {
  const { message, modal } = App.useApp();
  const profiles = useConnectionStore((state) => state.profiles);
  const loading = useConnectionStore((state) => state.loading);
  const remove = useConnectionStore((state) => state.remove);
  const markConnected = useConnectionStore((state) => state.markConnected);
  const openConnection = useUiStore((state) => state.openConnection);
  const setStatus = useUiStore((state) => state.setStatus);
  const openQuery = useTabStore((state) => state.openQuery);
  const openData = useTabStore((state) => state.openData);
  const openStructure = useTabStore((state) => state.openStructure);
  const openPlaceholder = useTabStore((state) => state.openPlaceholder);
  const [keyword, setKeyword] = useState("");
  const [cache, setCache] = useState<Record<string, XNode[]>>({});
  const [expandedKeys, setExpandedKeys] = useState<string[]>([]);
  const [selectedKeys, setSelectedKeys] = useState<string[]>([]);
  const loadedKeys = useMemo(() => Object.keys(cache), [cache]);
  const catalogRevisions = useRef<Record<string, number>>({});
  const previousProfiles = useRef(profiles);
  const [menu, setMenu] = useState<{ x: number; y: number; meta: NodeMeta } | null>(null);
  const [groupDialog, setGroupDialog] = useState<{ path: string; edit: GroupEdit } | null>(null);
  const [groupError, setGroupError] = useState("");
  const [savingGroup, setSavingGroup] = useState(false);
  const affectedCount = groupDialog
    ? profiles.filter((p) => inGroup(groupParts(p.group).join("/"), groupDialog.path)).length : 0;

  function clearConnectionTree(id: string) {
    catalogRevisions.current[id] = (catalogRevisions.current[id] ?? 0) + 1;
    setCache((previous) => Object.fromEntries(Object.entries(previous).filter(([key]) => !belongsToConnection(key, id))));
    setExpandedKeys((keys) => keys.filter((key) => !belongsToConnection(key, id)));
  }

  useEffect(() => {
    for (const previous of previousProfiles.current) {
      const next = profiles.find((profile) => profile.id === previous.id);
      if (sessionProfile(previous) === sessionProfile(next)) continue;
      // A successful initial handshake keeps its in-flight catalog read; edits and disconnects invalidate it.
      if (!next || (previous.connected && !next.connected)
        || JSON.stringify({ ...previous, connected: false }) !== JSON.stringify({ ...next, connected: false })) {
        clearConnectionTree(previous.id);
      }
    }
    previousProfiles.current = profiles;
  }, [profiles]);

  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    window.addEventListener("click", close);
    return () => window.removeEventListener("click", close);
  }, [menu]);

  const treeData = useMemo(
    () => buildTree(profiles, cache, keyword.trim().toLowerCase()),
    [profiles, cache, keyword],
  );

  async function ensureConnected(id: string) {
    const profile = useConnectionStore.getState().profiles.find((item) => item.id === id);
    if (!profile) throw new Error("连接不存在");
    if (!profile.connected) {
      await ensureConnection(id);
      touchRecent(id);
      setStatus(`已连接 ${profile.name}`);
    }
  }

  async function produceChildren(meta: NodeMeta, _key: string): Promise<XNode[]> {
    if (meta.kind === "connection" && meta.connectionId && meta.engine) {
      await ensureConnected(meta.connectionId);
      if (meta.engine === "sqlite") {
        return folderPair(meta.connectionId, meta.engine, "main", "main");
      }
      const databases = await api.listDatabases(meta.connectionId);
      return databases.map((database) => ({
        key: nodeKey("db", meta.connectionId!, database),
        title: database,
        icon: <Database size={14} />,
        isLeaf: false,
        meta: {
          kind: "database",
          connectionId: meta.connectionId,
          engine: meta.engine,
          database,
          searchText: database,
        },
      }));
    }
    if (meta.kind === "database" && meta.connectionId && meta.engine && meta.database) {
      if (usesSchema(meta.engine)) {
        const schemas = await api.listSchemas(meta.connectionId, meta.database);
        return schemas.map((schema) => ({
          key: nodeKey("schema", meta.connectionId!, meta.database!, schema),
          title: schema,
          icon: <Folder size={14} />,
          isLeaf: false,
          meta: {
            kind: "schema",
            connectionId: meta.connectionId,
            engine: meta.engine,
            database: meta.database,
            schema,
            searchText: schema,
          },
        }));
      }
      return folderPair(meta.connectionId, meta.engine, meta.database, meta.database);
    }
    if (meta.kind === "schema" && meta.connectionId && meta.engine && meta.database && meta.schema) {
      return folderPair(meta.connectionId, meta.engine, meta.database, meta.schema);
    }
    if (meta.kind === "folder" && meta.connectionId && meta.database && meta.schema && meta.folder) {
      const names =
        meta.folder === "views"
          ? await api.listViews(meta.connectionId, meta.database, meta.schema)
          : await api.listTables(meta.connectionId, meta.database, meta.schema);
      return names.map((name) => ({
        key: nodeKey(meta.folder!, meta.connectionId!, meta.database!, meta.schema!, name),
        title: name,
        icon: meta.folder === "views" ? <Eye size={14} /> : <Table2 size={14} />,
        isLeaf: false,
        meta: {
          kind: meta.folder === "views" ? "view" : "table",
          connectionId: meta.connectionId,
          engine: meta.engine,
          database: meta.database,
          schema: meta.schema,
          name,
          searchText: name,
        },
      }));
    }
    if ((meta.kind === "table" || meta.kind === "view") && meta.connectionId && meta.name) {
      const columns = await api.listColumns(
        meta.connectionId,
        meta.database ?? "",
        meta.schema ?? "",
        meta.name,
      );
      return columns.map((column) => ({
        key: nodeKey("col", meta.connectionId!, meta.database ?? "", meta.schema ?? "", meta.name!, column.name),
        title: (
          <span>
            {column.name} <span className="text-[var(--xb-muted)]">{column.dataType}</span>
            {column.primaryKey ? " PK" : ""}
          </span>
        ),
        isLeaf: true,
        meta: {
          kind: "column",
          connectionId: meta.connectionId,
          searchText: `${column.name} ${column.dataType}`,
        },
      }));
    }
    return [];
  }

  async function onLoad(node: EventDataNode<XNode>) {
    const key = String(node.key);
    if (cache[key]) return;
    const id = node.meta.connectionId ?? "";
    const revision = catalogRevisions.current[id] ?? 0;
    const current = () => revision === (catalogRevisions.current[id] ?? 0);
    try {
      const children = await produceChildren(node.meta, key);
      if (!current()) return;
      setCache((prev) => ({ ...prev, [key]: children }));
    } catch (err) {
      if (!current()) return;
      const text = errorText(err);
      setStatus(text);
      setCache((prev) => ({
        ...prev,
        [key]: [
          {
            key: `${key}${SEP}error`,
            title: text,
            isLeaf: true,
            meta: { kind: "note", searchText: text, connectionId: node.meta.connectionId },
          },
        ],
      }));
    }
  }

  function openObject(meta: NodeMeta, readOnly: boolean) {
    if (!meta.connectionId || !meta.name) return;
    openData({
      connectionId: meta.connectionId,
      database: meta.database ?? "",
      schema: meta.schema ?? "",
      table: meta.name,
      readOnly,
      title: meta.name,
    });
  }

  function newQuery(meta: NodeMeta) {
    const profile = profiles.find((item) => item.id === meta.connectionId);
    const sql =
      meta.name && meta.engine
        ? `SELECT * FROM ${qualifiedTable(meta.engine, meta.database ?? "", meta.schema ?? "", meta.name)};\n`
        : undefined;
    openQuery({
      connectionId: meta.connectionId ?? profile?.id ?? null,
      database: meta.database || (profile?.engine === "sqlite" ? "main" : profile?.database || ""),
      sql,
      title: meta.name ? `查询 ${meta.name}` : "查询",
    });
  }

  async function removeConnection(id: string) {
    modal.confirm({
      title: "删除这个连接？",
      content: "只删除本机保存的连接配置，不会删除数据库里的数据。",
      okText: "删除",
      okButtonProps: { danger: true },
      cancelText: "取消",
      onOk: async () => {
        await api.deleteConnection(id);
        remove(id);
        setStatus("连接已删除");
      },
    });
  }

  const menuItems = menu ? actionsFor(menu.meta) : [];

  function editGroup(path: string, edit: GroupEdit) {
    setGroupError("");
    setGroupDialog({ path, edit });
  }

  async function saveGroup() {
    if (!groupDialog || savingGroup) return;
    const { path, edit } = groupDialog;
    setSavingGroup(true);
    try {
      const changes = await api.editConnectionGroup(path, edit);
      const parent = path.split("/").slice(0, -1).join("/");
      const hasDirect = Object.values(changes).includes(parent);
      useConnectionStore.getState().applyGroups(changes);
      setExpandedKeys((keys) => remapGroupKeys(keys, path, edit, hasDirect));
      setSelectedKeys((keys) => remapGroupKeys(keys, path, edit, hasDirect));
      setGroupDialog(null);
      setStatus(edit.kind === "rename" ? "分组已重命名" : "分组已解散");
    } catch (err) {
      setGroupError(errorText(err));
    } finally {
      setSavingGroup(false);
    }
  }

  function actionsFor(meta: NodeMeta): { label: string; run: () => void }[] {
    if (meta.kind === "group" && meta.groupPath !== undefined) {
      const path = meta.groupPath;
      const key = nodeKey("group", path);
      return [
        { label: "在此分组新建连接", run: () => openConnection(null, path) },
        ...(path ? [
          { label: "重命名分组", run: () => editGroup(path, { kind: "rename", name: path.split("/").pop()! }) },
          { label: "解散分组", run: () => editGroup(path, { kind: "dissolve" }) },
        ] : []),
        { label: expandedKeys.includes(key) ? "收起分组" : "展开分组", run: () =>
          setExpandedKeys((keys) => keys.includes(key) ? keys.filter((item) => item !== key) : [...keys, key]) },
      ];
    }
    if (meta.kind === "connection" && meta.connectionId) {
      const id = meta.connectionId;
      return [
        {
          label: "连接",
          run: () => {
            void ensureConnected(id).catch((err) => message.error(errorText(err)));
          },
        },
        {
          label: "断开",
          run: () => {
            void api.disconnect(id).then(() => {
              markConnected(id, false);
              setStatus("已断开");
            });
          },
        },
        { label: "新建查询", run: () => newQuery(meta) },
        { label: "备份", run: () => openTool("backup", meta) },
        { label: "编辑", run: () => openConnection(id) },
        { label: "删除", run: () => void removeConnection(id) },
        {
          label: "刷新对象",
          run: () => {
            invalidateSqlCatalog(id);
            clearConnectionTree(id);
          },
        },
      ];
    }
    if (meta.kind === "table" || meta.kind === "view") {
      return [
        { label: "打开数据", run: () => openObject(meta, meta.kind === "view") },
        {
          label: "查看结构",
          run: () => {
            if (!meta.connectionId || !meta.name) return;
            openStructure({
              connectionId: meta.connectionId,
              database: meta.database ?? "",
              schema: meta.schema ?? "",
              table: meta.name,
            });
          },
        },
        { label: "新建查询", run: () => newQuery(meta) },
        { label: "导入导出", run: () => openTool("io", meta, meta.name) },
        { label: "作为同步源", run: () => openTool("data-sync", meta, meta.name) },
        { label: "加入结构对比", run: () => openTool("schema-sync", meta, meta.name) },
      ];
    }
    if (meta.kind === "database" || meta.kind === "schema") {
      return [
        { label: "新建查询", run: () => newQuery(meta) },
        { label: "ER 图", run: () => openTool("er", meta) },
        { label: "备份", run: () => openTool("backup", meta) },
        { label: "库内搜索", run: () => openTool("search", meta) },
      ];
    }
    return [];
  }

  function openTool(feature: string, meta: NodeMeta, table = "") {
    if (!meta.connectionId) return;
    const profile = profiles.find((item) => item.id === meta.connectionId);
    const database = meta.database || (profile?.engine === "sqlite" ? "main" : profile?.database || "");
    const schema = meta.schema || (profile?.engine === "sqlite" ? "main" : usesSchema(profile?.engine) ? "" : database);
    const context: StudioContext = { connectionId: meta.connectionId, database, schema, table };
    openPlaceholder(feature, context);
  }

  return (
    <aside className="xb-sidebar" style={{ width }}>
      <div className="flex items-center justify-between px-3 pb-1 pt-3">
        <span className="text-xs text-[var(--xb-muted)]">连接</span>
        <Button size="small" type="text" onClick={() => openConnection(null)}>
          新建
        </Button>
      </div>
      <div className="px-3 pb-2">
        <Input
          allowClear
          size="small"
          placeholder="过滤连接或已加载对象"
          value={keyword}
          onChange={(event) => setKeyword(event.target.value)}
        />
      </div>
      <div className="min-h-0 flex-1 overflow-auto px-1 pb-3">
        {loading && profiles.length === 0 ? (
          <div className="px-3 py-6 text-center">
            <Spin size="small" />
          </div>
        ) : treeData.length === 0 ? (
          <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="还没有连接" />
        ) : (
          <Tree
            className="xb-tree"
            blockNode
            showIcon
            // Keep the clicked row under the pointer between the two clicks of a double-click.
            motion={false}
            treeData={treeData}
            expandedKeys={expandedKeys}
            loadedKeys={loadedKeys}
            selectedKeys={selectedKeys}
            onExpand={(keys) => setExpandedKeys(keys.map(String))}
            loadData={onLoad}
            onDoubleClick={(_, node) => {
              const meta = (node as XNode).meta;
              if (meta.kind === "table" || meta.kind === "view") openObject(meta, meta.kind === "view");
              if (meta.kind === "connection" && meta.connectionId) {
                void ensureConnected(meta.connectionId).catch((err) => message.error(errorText(err)));
              }
            }}
            onRightClick={({ event, node }) => {
              const meta = (node as XNode).meta;
              if (actionsFor(meta).length === 0) return;
              setMenu({
                x: Math.min(event.clientX, window.innerWidth - 190),
                y: Math.min(event.clientY, window.innerHeight - 240),
                meta,
              });
            }}
            onSelect={(_, info) => {
              const meta = (info.node as XNode).meta;
              const key = String(info.node.key);
              setSelectedKeys([key]);
              // Keyboard arrows use onExpand. A double-click has already toggled on its first click.
              if (info.nativeEvent?.type === "click" && info.nativeEvent.detail <= 1 && !info.node.isLeaf) {
                setExpandedKeys((keys) => keys.includes(key) ? keys.filter((item) => item !== key) : [...keys, key]);
              }
              if (meta.searchText) setStatus(meta.searchText);
            }}
          />
        )}
      </div>
      {menu && menuItems.length > 0 && (
        <div className="xb-menu" style={{ left: menu.x, top: menu.y }}>
          {menuItems.map((item) => (
            <button
              key={item.label}
              type="button"
              onClick={() => {
                setMenu(null);
                item.run();
              }}
            >
              {item.label}
            </button>
          ))}
        </div>
      )}
      {groupDialog && (
        <Modal
          open
          title={groupDialog.edit.kind === "rename" ? "重命名分组" : "解散分组"}
          okText={groupDialog.edit.kind === "rename" ? "保存" : "解散"}
          cancelText="取消"
          okButtonProps={{ danger: groupDialog.edit.kind === "dissolve" }}
          confirmLoading={savingGroup}
          onOk={() => void saveGroup()}
          onCancel={() => { if (!savingGroup) setGroupDialog(null); }}
        >
          <p>分组“{groupDialog.path}”及其子分组共包含 {affectedCount} 个连接。</p>
          {groupDialog.edit.kind === "rename" ? (
            <Input
              aria-label="分组名称"
              autoFocus
              disabled={savingGroup}
              value={groupDialog.edit.name}
              onChange={(event) => editGroup(groupDialog.path, { kind: "rename", name: event.target.value })}
              onPressEnter={() => void saveGroup()}
            />
          ) : (
            <p>解散后，直接连接将移至“{groupDialog.path.split("/").slice(0, -1).join("/") || "未分组"}”，子分组上移一级。</p>
          )}
          {groupError && <Alert className="mt-2" type="error" showIcon message={groupError} />}
        </Modal>
      )}
    </aside>
  );
}

function folderPair(connectionId: string, engine: Engine, database: string, schema: string): XNode[] {
  return [
    {
      key: nodeKey("folder", connectionId, database, schema, "tables"),
      title: "表",
      icon: <Folder size={14} />,
      isLeaf: false,
      meta: {
        kind: "folder",
        folder: "tables",
        connectionId,
        engine,
        database,
        schema,
        searchText: "表",
      },
    },
    {
      key: nodeKey("folder", connectionId, database, schema, "views"),
      title: "视图",
      icon: <Folder size={14} />,
      isLeaf: false,
      meta: {
        kind: "folder",
        folder: "views",
        connectionId,
        engine,
        database,
        schema,
        searchText: "视图",
      },
    },
  ];
}

function buildTree(profiles: ConnectionPublic[], cache: Record<string, XNode[]>, keyword: string): XNode[] {
  const root: GroupNode = { name: "", children: new Map(), profiles: [] };
  for (const profile of profiles) {
    const parts = groupParts(profile.group);
    if (!parts.length) parts.push("");
    let cursor = root;
    for (const part of parts) {
      const next = cursor.children.get(part) ?? { name: part, children: new Map(), profiles: [] };
      cursor.children.set(part, next);
      cursor = next;
    }
    cursor.profiles.push(profile);
  }
  return Array.from(root.children.values())
    .sort((a, b) => a.name.localeCompare(b.name, "zh-CN"))
    .map((node) => groupNode(node, "", cache, keyword))
    .filter((node) => (node.children?.length ?? 0) > 0);
}

interface GroupNode {
  name: string;
  children: Map<string, GroupNode>;
  profiles: ConnectionPublic[];
}

function groupNode(node: GroupNode, parent: string, cache: Record<string, XNode[]>, keyword: string): XNode {
  const path = parent ? `${parent}/${node.name}` : node.name;
  const folders = Array.from(node.children.values())
    .sort((a, b) => a.name.localeCompare(b.name, "zh-CN"))
    .map((child) => groupNode(child, path, cache, keyword));
  const connections = node.profiles
    .slice()
    .sort((a, b) => a.name.localeCompare(b.name, "zh-CN"))
    .map((profile) => connectionNode(profile, cache))
    .filter((item) => !keyword || treeMatches(item, keyword));
  return {
    key: nodeKey("group", path),
    title: path === "未分组" ? "未分组（自定义）" : node.name || "未分组",
    meta: { kind: "group", groupPath: path, searchText: path || "未分组" },
    children: [...folders, ...connections].filter((item) => !keyword || treeMatches(item, keyword) || (item.children?.length ?? 0) > 0),
  };
}

function connectionNode(profile: ConnectionPublic, cache: Record<string, XNode[]>): XNode {
  const key = nodeKey("conn", profile.id);
  return {
    key,
    isLeaf: false,
    meta: {
      kind: "connection",
      connectionId: profile.id,
      engine: profile.engine,
      database: profile.engine === "sqlite" ? "main" : profile.database,
      searchText: `${profile.name} ${engineLabel(profile.engine)} ${profile.host} ${profile.filePath}`,
    },
    title: (
      <span className="xb-conn">
        <EngineGlyph engine={profile.engine} size={16} />
        <i className="xb-dot" style={{ background: profile.color || "var(--xb-accent)" }} />
        <span>{profile.name}</span>
        <span className="xb-conn-engine">{engineLabel(profile.engine)}</span>
        {profile.connected && <span className="xb-live">在线</span>}
      </span>
    ),
    children: attach(cache, key),
  };
}

function attach(cache: Record<string, XNode[]>, key: string): XNode[] | undefined {
  const children = cache[key];
  if (!children) return undefined;
  return children.map((child) => ({
    ...child,
    children: child.isLeaf ? child.children : attach(cache, String(child.key)),
  }));
}

function treeMatches(node: XNode, keyword: string): boolean {
  if (node.meta.searchText.toLowerCase().includes(keyword)) return true;
  return (node.children ?? []).some((child) => treeMatches(child as XNode, keyword));
}
