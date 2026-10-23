export * from "./generated/contracts";

export interface QueryTab {
  id: string;
  kind: "query";
  title: string;
  connectionId: string | null;
  database: string;
  sql: string;
}

export interface DataTab {
  id: string;
  kind: "data";
  title: string;
  connectionId: string;
  database: string;
  schema: string;
  table: string;
  readOnly: boolean;
}

export interface StructureTab {
  id: string;
  kind: "structure";
  title: string;
  connectionId: string;
  database: string;
  schema: string;
  table: string;
}

export interface StudioContext {
  connectionId: string;
  database: string;
  schema: string;
  table: string;
}

export interface PlaceholderTab {
  id: string;
  kind: "placeholder";
  title: string;
  feature: string;
  summary: string;
  points: string[];
  context?: StudioContext;
}

export type WorkspaceTab = QueryTab | DataTab | StructureTab | PlaceholderTab;
