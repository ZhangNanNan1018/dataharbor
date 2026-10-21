import { create } from "zustand";
import { placeholderById } from "../placeholders";
import type { DataTab, QueryTab, StructureTab, StudioContext, WorkspaceTab } from "../types";

interface OpenQueryInput {
  connectionId?: string | null;
  database?: string;
  sql?: string;
  title?: string;
}

interface OpenDataInput {
  connectionId: string;
  database: string;
  schema: string;
  table: string;
  readOnly: boolean;
  title?: string;
}

interface TabState {
  tabs: WorkspaceTab[];
  activeId: string | null;
  dirty: Record<string, boolean>;
  draftSaveError: string | null;
  openQuery: (input?: OpenQueryInput) => string;
  openData: (input: OpenDataInput) => string;
  openStructure: (input: Omit<OpenDataInput, "readOnly">) => string;
  openPlaceholder: (feature: string, context?: StudioContext) => string;
  close: (id: string) => void;
  setActive: (id: string) => void;
  patchQuery: (id: string, patch: Partial<Pick<QueryTab, "connectionId" | "database" | "sql" | "title">>) => void;
  setDirty: (id: string, dirty: boolean) => void;
}

function uid(): string {
  return crypto.randomUUID();
}

const DRAFTS_KEY = "dataharbor-query-workspace-v1";

function readDraftWorkspace(): Pick<TabState, "tabs" | "activeId" | "dirty" | "draftSaveError"> {
  try {
    const raw = localStorage.getItem(DRAFTS_KEY);
    if (!raw) return { tabs: [], activeId: null, dirty: {}, draftSaveError: null };
    const saved = JSON.parse(raw) as { version?: number; tabs?: unknown; activeId?: unknown; dirty?: Record<string, unknown> };
    if (saved.version !== 1 || !Array.isArray(saved.tabs)) throw new Error("无法读取查询草稿");
    const seen = new Set<string>();
    const tabs = saved.tabs.filter((value): value is QueryTab => {
      if (!value || typeof value !== "object") return false;
      const tab = value as QueryTab;
      if (tab.kind !== "query" || typeof tab.id !== "string" || !tab.id || seen.has(tab.id)
        || typeof tab.title !== "string" || typeof tab.sql !== "string" || typeof tab.database !== "string"
        || !(tab.connectionId === null || typeof tab.connectionId === "string")) return false;
      seen.add(tab.id);
      return true;
    });
    if (tabs.length !== saved.tabs.length) throw new Error("查询草稿包含损坏或重复记录");
    return {
      tabs,
      activeId: tabs.some((tab) => tab.id === saved.activeId) ? String(saved.activeId) : tabs[0]?.id ?? null,
      dirty: Object.fromEntries(tabs.map((tab) => [tab.id, saved.dirty?.[tab.id] !== false])),
      draftSaveError: null,
    };
  } catch {
    return { tabs: [], activeId: null, dirty: {}, draftSaveError: "查询草稿恢复失败；原始恢复资料仍保留，请勿关闭需要保留的查询。" };
  }
}

export const useTabStore = create<TabState>((set, get) => ({
  ...readDraftWorkspace(),
  openQuery: (input) => {
    const id = uid();
    const tab: QueryTab = {
      id,
      kind: "query",
      title: input?.title || "查询",
      connectionId: input?.connectionId ?? null,
      database: input?.database ?? "",
      sql: input?.sql ?? "-- Ctrl+Enter 运行\nSELECT 1;\n",
    };
    set({ tabs: [...get().tabs, tab], activeId: id, dirty: { ...get().dirty, [id]: Boolean(input?.sql) } });
    return id;
  },
  openData: (input) => {
    const existing = get().tabs.find(
      (tab): tab is DataTab =>
        tab.kind === "data" &&
        tab.connectionId === input.connectionId &&
        tab.database === input.database &&
        tab.schema === input.schema &&
        tab.table === input.table,
    );
    if (existing) {
      set({ activeId: existing.id });
      return existing.id;
    }
    const id = uid();
    const tab: DataTab = {
      id,
      kind: "data",
      title: input.title || input.table,
      connectionId: input.connectionId,
      database: input.database,
      schema: input.schema,
      table: input.table,
      readOnly: input.readOnly,
    };
    set({ tabs: [...get().tabs, tab], activeId: id });
    return id;
  },
  openStructure: (input) => {
    const existing = get().tabs.find(
      (tab): tab is StructureTab =>
        tab.kind === "structure" &&
        tab.connectionId === input.connectionId &&
        tab.database === input.database &&
        tab.schema === input.schema &&
        tab.table === input.table,
    );
    if (existing) {
      set({ activeId: existing.id });
      return existing.id;
    }
    const id = uid();
    const tab: StructureTab = {
      id,
      kind: "structure",
      title: `结构 · ${input.table}`,
      connectionId: input.connectionId,
      database: input.database,
      schema: input.schema,
      table: input.table,
    };
    set({ tabs: [...get().tabs, tab], activeId: id });
    return id;
  },
  openPlaceholder: (feature, context) => {
    const spec = placeholderById(feature);
    const existing = get().tabs.find((tab) => tab.kind === "placeholder" && tab.feature === feature);
    if (existing) {
      set({
        activeId: existing.id,
        tabs: context
          ? get().tabs.map((tab) => (tab.id === existing.id && tab.kind === "placeholder" ? { ...tab, context } : tab))
          : get().tabs,
      });
      return existing.id;
    }
    const id = uid();
    set({
      tabs: [
        ...get().tabs,
        {
          id,
          kind: "placeholder",
          title: spec.title,
          feature: spec.id,
          summary: spec.summary,
          points: spec.points,
          context,
        },
      ],
      activeId: id,
    });
    return id;
  },
  close: (id) => {
    const tabs = get().tabs.filter((tab) => tab.id !== id);
    const dirty = { ...get().dirty };
    delete dirty[id];
    const activeId = get().activeId === id ? tabs.at(-1)?.id ?? null : get().activeId;
    set({ tabs, activeId, dirty });
  },
  setActive: (id) => set({ activeId: id }),
  patchQuery: (id, patch) => {
    const previous = get().tabs.find((tab) => tab.id === id && tab.kind === "query") as QueryTab | undefined;
    set({
      tabs: get().tabs.map((tab) => (tab.id === id && tab.kind === "query" ? { ...tab, ...patch } : tab)),
      dirty: previous && patch.sql !== undefined && patch.sql !== previous.sql ? { ...get().dirty, [id]: true } : get().dirty,
    });
  },
  setDirty: (id, dirty) =>
    set((state) => {
      if (Boolean(state.dirty[id]) === dirty) return state;
      return { dirty: { ...state.dirty, [id]: dirty } };
    }),
}));

useTabStore.subscribe((state, previous) => {
  if (state.tabs === previous.tabs && state.activeId === previous.activeId && state.dirty === previous.dirty) return;
  // Do not overwrite an unreadable recovery record with an empty workspace.
  if (state.draftSaveError?.startsWith("查询草稿恢复失败")) return;
  const tabs = state.tabs.filter((tab) => tab.kind === "query");
  try {
    localStorage.setItem(DRAFTS_KEY, JSON.stringify({ version: 1, tabs, activeId: state.activeId, dirty: state.dirty }));
    if (state.draftSaveError) useTabStore.setState({ draftSaveError: null });
  } catch {
    if (!state.draftSaveError) useTabStore.setState({ draftSaveError: "查询草稿自动保存失败，请复制需要保留的 SQL 后再关闭应用。" });
  }
});
