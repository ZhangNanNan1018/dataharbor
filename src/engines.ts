import type { Engine } from "./types";

export interface EngineOption {
  value: Engine;
  label: string;
  port: number;
  placeholder?: boolean;
}

export const ENGINES: EngineOption[] = [
  { value: "mysql", label: "MySQL", port: 3306 },
  { value: "mariadb", label: "MariaDB", port: 3306 },
  { value: "postgres", label: "PostgreSQL", port: 5432 },
  { value: "sqlite", label: "SQLite", port: 0 },
  { value: "mongodb", label: "MongoDB", port: 27017 },
  { value: "redis", label: "Redis", port: 6379 },
  { value: "mssql", label: "SQL Server", port: 1433 },
  { value: "oracle", label: "Oracle", port: 1521 },
  { value: "snowflake", label: "Snowflake", port: 443 },
  { value: "dameng", label: "达梦", port: 5236 },
];

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
