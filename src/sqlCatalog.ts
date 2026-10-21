import { api } from "./api";
import type { SchemaTable } from "./sqlIntelligence";

interface CatalogContext { connectionId: string; database: string; schema: string }
interface CatalogEntry {
  context: CatalogContext;
  tables: SchemaTable[];
  listeners: Set<(tables: SchemaTable[]) => void>;
  started: boolean;
  revision: number;
}
const catalogs = new Map<string, CatalogEntry>();

function publish(entry: CatalogEntry) {
  for (const listener of entry.listeners) listener(entry.tables);
}

async function load(entry: CatalogEntry) {
  entry.started = true;
  const revision = entry.revision;
  const current = () => entry.revision === revision;
  const { connectionId, database, schema } = entry.context;
  try {
    const names = await api.listTables(connectionId, database, schema);
    if (!current()) return;
    entry.tables = names.map((name) => ({ name, columns: [], columnsLoaded: false }));
    publish(entry);
    // Bound catalog work and publish table names before waiting for remote column reads.
    for (let start = 0; start < Math.min(names.length, 40); start += 4) {
      const batch = await Promise.all(names.slice(start, Math.min(start + 4, 40)).map(async (name) => {
        try {
          return { name, columns: await api.listColumns(connectionId, database, schema, name), columnsLoaded: true };
        } catch { return null; }
      }));
      if (!current()) return;
      entry.tables = entry.tables.map((table) => batch.find((item) => item?.name === table.name) ?? table);
      publish(entry);
    }
  } catch {
    if (current()) {
      entry.started = false;
      entry.tables = [];
      publish(entry);
    }
  }
}

/** A catalog snapshot is shared by editors; unsubscribing only releases that editor. */
export function watchSqlCatalog(context: CatalogContext, listener: (tables: SchemaTable[]) => void): () => void {
  const key = JSON.stringify([context.connectionId, context.database, context.schema]);
  let entry = catalogs.get(key);
  if (!entry) {
    entry = { context, tables: [], listeners: new Set(), started: false, revision: 0 };
    catalogs.set(key, entry);
  }
  entry.listeners.add(listener);
  listener(entry.tables);
  if (!entry.started) void load(entry);
  return () => { entry.listeners.delete(listener); };
}

export function invalidateSqlCatalog(connectionId: string, reload = true) {
  for (const [key, entry] of catalogs) {
    if (entry.context.connectionId !== connectionId) continue;
    entry.revision += 1;
    entry.tables = [];
    entry.started = false;
    if (entry.listeners.size) { publish(entry); if (reload) void load(entry); }
    else catalogs.delete(key);
  }
}
