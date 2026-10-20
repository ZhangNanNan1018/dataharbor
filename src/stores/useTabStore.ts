import { create } from "zustand";
import { placeholderById } from "../placeholders";
import type { DataTab, QueryTab, StructureTab, WorkspaceTab } from "../types";

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
  openQuery: (input?: OpenQueryInput) => string;
  openData: (input: OpenDataInput) => string;
  openStructure: (input: Omit<OpenDataInput, "readOnly">) => string;
  openPlaceholder: (feature: string) => string;
  close: (id: string) => void;
  setActive: (id: string) => void;
  patchQuery: (id: string, patch: Partial<Pick<QueryTab, "connectionId" | "database" | "sql" | "title">>) => void;
  setDirty: (id: string, dirty: boolean) => void;
}

function uid(): string {
  return crypto.randomUUID();
}

export const useTabStore = create<TabState>((set, get) => ({
  tabs: [],
  activeId: null,
  dirty: {},
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
    set({ tabs: [...get().tabs, tab], activeId: id });
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
  openPlaceholder: (feature) => {
    const spec = placeholderById(feature);
    const existing = get().tabs.find((tab) => tab.kind === "placeholder" && tab.feature === feature);
    if (existing) {
      set({ activeId: existing.id });
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
  patchQuery: (id, patch) =>
    set({
      tabs: get().tabs.map((tab) => (tab.id === id && tab.kind === "query" ? { ...tab, ...patch } : tab)),
    }),
  setDirty: (id, dirty) =>
    set((state) => {
      if (Boolean(state.dirty[id]) === dirty) return state;
      return { dirty: { ...state.dirty, [id]: dirty } };
    }),
}));
