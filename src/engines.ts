import type { Engine, EngineCapabilities } from "./types";
import { ENGINE_CAPABILITIES } from "./generated/contracts";

export interface EngineOption {
  value: Engine;
  label: string;
  port: number;
  placeholder?: boolean;
}

export const ENGINES: EngineOption[] = ENGINE_CAPABILITIES.map((item) => ({ value: item.engine, label: item.label, port: item.defaultPort }));
export function engineCapabilities(engine: Engine | undefined): EngineCapabilities | undefined {
  return ENGINE_CAPABILITIES.find((item) => item.engine === engine);
}

export const COLORS = [
  "#6366f1",
  "#e36d6d",
  "#3cba8e",
  "#5b8def",
  "#b07be0",
  "#e07bb0",
  "#7d8b9c",
  "#46b5c4",
];

export function engineLabel(engine: Engine | undefined): string {
  return ENGINES.find((item) => item.value === engine)?.label ?? engine ?? "";
}

export function isServerEngine(engine: Engine): boolean {
  return engine !== "sqlite";
}

/** Engines whose object tree has a schema level between database and table. */
export function usesSchema(engine: Engine | undefined): boolean {
  return engineCapabilities(engine)?.schemaTree ?? false;
}

export function quoteIdent(engine: Engine | undefined, name: string): string {
  if (engine === "mysql" || engine === "mariadb") {
    return `\`${name.replaceAll("`", "``")}\``;
  }
  if (engine === "mssql") {
    return `[${name.replaceAll("]", "]]")}]`;
  }
  return `"${name.replaceAll('"', '""')}"`;
}

export function qualifiedTable(
  engine: Engine | undefined,
  database: string,
  schema: string,
  table: string,
): string {
  const quoted = quoteIdent(engine, table);
  if (engine === "mysql" || engine === "mariadb") {
    return database ? `${quoteIdent(engine, database)}.${quoted}` : quoted;
  }
  if (engine === "dameng") {
    return schema ? `${quoteIdent(engine, schema)}.${quoted}` : quoted;
  }
  if (engine === "postgres" || engine === "oracle" || engine === "snowflake") {
    const fallback = engine === "oracle" ? "USER" : engine === "snowflake" ? "PUBLIC" : "public";
    return `${quoteIdent(engine, schema || fallback)}.${quoted}`;
  }
  if (engine === "mssql") {
    return `${quoteIdent(engine, schema || "dbo")}.${quoted}`;
  }
  return quoted;
}
