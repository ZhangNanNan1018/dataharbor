import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import type {
  AppInfo,
  ColumnMeta,
  CommitResult,
  ConnectResult,
  ConnectionPublic,
  DataPage,
  ExecuteSqlResponse,
  ExportResult,
  FetchRowsRequest,
  JsonValue,
  FilterSpec,
  ImportResult,
  RowChange,
  SaveConnectionRequest,
  SaveConnectionResult,
  SortSpec,
  TestResult,
} from "./types";

export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export function errorText(err: unknown): string {
  if (typeof err === "string") return err;
  if (err instanceof Error) return err.message;
  if (err && typeof err === "object" && "message" in err) {
    const message = (err as { message?: unknown }).message;
    if (typeof message === "string") return message;
  }
  return "未知错误";
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) {
    throw new Error("当前在浏览器预览中。请在桌面窗口里使用：npm run tauri dev");
  }
  return invoke<T>(command, args);
}

export interface OperationSnapshot {
  id: string;
  label: string;
  status: "queued" | "running" | "succeeded" | "failed" | "cancelled";
  phase: string;
  completed: number;
  total: number | null;
  committed: number;
  batches: number;
  cancelRequested: boolean;
  message: string;
  interruptible: boolean;
}

export function isOperationCancelled(error: unknown): boolean {
  return Boolean(error && typeof error === "object" && "kind" in error && error.kind === "cancelled");
}

export const api = {
  createOperation: (label: string) => call<OperationSnapshot>("create_operation", { label }),
  operationStatus: (id: string) => call<OperationSnapshot>("operation_status", { id }),
  cancelOperation: (id: string) => call<OperationSnapshot>("cancel_operation", { id }),
  releaseOperation: (id: string) => call<void>("release_operation", { id }),
  appInfo: () => call<AppInfo>("app_info"),
  listConnections: () => call<ConnectionPublic[]>("list_connections"),
  saveConnection: (request: SaveConnectionRequest) =>
    call<SaveConnectionResult>("save_connection", { request }),
  deleteConnection: (id: string) => call<void>("delete_connection", { id }),
  testConnection: (request: SaveConnectionRequest) =>
    call<TestResult>("test_connection", { request }),
  connect: (id: string) => call<ConnectResult>("connect", { id }),
  disconnect: (id: string) => call<void>("disconnect", { id }),
  createSample: () => call<ConnectionPublic>("create_sample_database"),
  listDatabases: (connectionId: string) => call<string[]>("list_databases", { connectionId }),
  listSchemas: (connectionId: string, database: string) =>
    call<string[]>("list_schemas", { connectionId, database }),
  listTables: (connectionId: string, database: string, schema: string) =>
    call<string[]>("list_tables", { connectionId, database, schema }),
  listViews: (connectionId: string, database: string, schema: string) =>
    call<string[]>("list_views", { connectionId, database, schema }),
  listColumns: (connectionId: string, database: string, schema: string, objectName: string) =>
    call<ColumnMeta[]>("list_columns", { connectionId, database, schema, objectName }),
  executeSql: (request: {
    connectionId: string;
    database: string;
    sql: string;
    page: number;
    pageSize: number;
    onlyStatement?: number | null;
  }, operationId?: string) => call<ExecuteSqlResponse>("execute_sql", { request, operationId }),
  fetchRows: (request: FetchRowsRequest) => call<DataPage>("fetch_rows", { request }),
  commitChanges: (request: {
    connectionId: string;
    database: string;
    schema: string;
    table: string;
    changes: RowChange[];
  }) => call<CommitResult>("commit_changes", { request }),
  exportTableCsv: (request: {
    connectionId: string;
    database: string;
    schema: string;
    table: string;
    filters: FilterSpec[];
    sorts: SortSpec[];
    path: string;
  }, operationId?: string) => call<ExportResult>("export_table_csv", { request, operationId }),
  exportSqlCsv: (request: { connectionId: string; database: string; sql: string; path: string }, operationId?: string) =>
    call<ExportResult>("export_sql_csv", { request, operationId }),
  importTableCsv: (request: {
    connectionId: string;
    database: string;
    schema: string;
    table: string;
    path: string;
    truncate: boolean;
  }, operationId?: string) => call<ImportResult>("import_table_csv", { request, operationId }),
  formatSql: (sql: string, minify: boolean) => call<string>("format_sql", { request: { sql, minify } }),
  bindSql: (sql: string, engine: string, params: { name: string; value: unknown }[]) =>
    call<string>("bind_sql", { request: { sql, engine, params } }),
  explainQuery: (connectionId: string, database: string, sql: string) =>
    call<GridResult>("explain_query", { connectionId, database, sql }),
  listSnippets: () => call<Snippet[]>("list_snippets"),
  saveSnippet: (snippet: Partial<Snippet> & { name: string; body: string }) =>
    call<Snippet>("save_snippet", { snippet }),
  deleteSnippet: (id: string) => call<void>("delete_snippet", { id }),
  erDiagram: (request: ObjectRef) => call<ErGraph>("er_diagram", { request }),
  exportTableFile: (request: FileTransferRequest, operationId?: string) => call<FileExportResult>("export_table_file", { request, operationId }),
  importTableFile: (request: FileTransferRequest, operationId?: string) => call<number>("import_table_file", { request, operationId }),
  diffStructure: (request: PairRequest) => call<DiffResult>("diff_structure", { request }),
  transferRows: (request: PairRequest, operationId?: string) => call<SyncResult>("transfer_rows", { request, operationId }),
  syncRows: (request: PairRequest, operationId?: string) => call<SyncResult>("sync_rows", { request, operationId }),
  backupSql: (request: BackupRequest, operationId?: string) => call<FileExportResult>("backup_sql", { request, operationId }),
  restoreSql: (request: RestoreRequest, operationId?: string) => call<string>("restore_sql", { request, operationId }),
  generateRows: (request: TableRef & { count: number }) => call<number>("generate_rows", { request }),
  maskColumns: (request: TableRef & { columns: string[] }) => call<string>("mask_columns", { request }),
  dataDictionary: (request: ObjectRef) => call<string>("data_dictionary", { request }),
  analyzeSchema: (request: ObjectRef) => call<GridResult>("analyze_schema", { request }),
  listAccounts: (connectionId: string, database: string) =>
    call<GridResult>("list_accounts", { connectionId, database }),
  serverActivity: (connectionId: string, database: string) =>
    call<GridResult>("server_activity", { connectionId, database }),
  previewDesign: (request: Record<string, unknown>) => call<string>("preview_design", { request }),
  aiSettings: () => call<AiSettings>("ai_settings"),
  saveAiSettings: (request: AiSettingsRequest) => call<AiSettings>("save_ai_settings", { request }),
  aiAsk: (request: AiAskRequest) => call<string>("ai_ask", { request }),
  listJobs: () => call<Job[]>("list_jobs"),
  saveJob: (job: Job) => call<Job>("save_job", { job }),
  deleteJob: (id: string) => call<void>("delete_job", { id }),
  runDueJobs: () => call<string[]>("run_due_jobs"),
  listDashboards: () => call<Dashboard[]>("list_dashboards"),
  saveDashboard: (dashboard: Dashboard) => call<Dashboard>("save_dashboard", { dashboard }),
  deleteDashboard: (id: string) => call<void>("delete_dashboard", { id }),
  listModels: () => call<DataModel[]>("list_models"),
  saveModel: (model: DataModel) => call<DataModel>("save_model", { model }),
  deleteModel: (id: string) => call<void>("delete_model", { id }),
  reverseModel: (request: ObjectRef, name: string) => call<DataModel>("reverse_model", { request, name }),
  forwardSql: (model: DataModel, engine: string) => call<string>("forward_sql", { model, engine }),
  modelHtml: (model: DataModel) => call<string>("model_html", { model }),
  listFavorites: () => call<Favorite[]>("list_favorites"),
  saveFavorite: (favorite: Partial<Favorite>) => call<Favorite>("save_favorite", { favorite }),
  deleteFavorite: (id: string) => call<void>("delete_favorite", { id }),
  searchCatalog: (request: ObjectRef, query: string) => call<GridResult>("search_catalog", { request, query }),
  redisPublish: (connectionId: string, channel: string, message: string) =>
    call<number>("redis_publish", { connectionId, channel, message }),
  redisListen: (connectionId: string, channel: string, waitMs: number) =>
    call<string[]>("redis_listen", { connectionId, channel, waitMs }),
  exportProject: (path: string) => call<string>("export_project", { path }),
  importProject: (path: string) => call<string>("import_project", { path }),
  explainPlan: (connectionId: string, database: string, sql: string) =>
    call<PlanNode>("explain_plan", { connectionId, database, sql }),
  foreignKeys: (connectionId: string, database: string, schema: string, table: string) =>
    call<ForeignKeyRef[]>("foreign_keys", { connectionId, database, schema, table }),
  referenceRows: (connectionId: string, database: string, schema: string, table: string, column: string, query: string) =>
    call<GridResult>("reference_rows", { connectionId, database, schema, table, column, query }),
  debugStatus: (connectionId: string, database: string) =>
    call<DebugReport>("debug_status", { connectionId, database }),
  debugLaunch: (connectionId: string, database: string, functionName: string, callSql: string, line: number) =>
    call<DebugSnapshot>("debug_launch", { connectionId, database, function: functionName, callSql, line }),
  debugStep: (connectionId: string, action: string) =>
    call<DebugSnapshot>("debug_step", { connectionId, action }),
  debugStop: (connectionId: string) => call<DebugSnapshot>("debug_stop", { connectionId }),
  profileTable: (request: ObjectRef) => call<GridResult>("profile_table", { request }),
  redisExpire: (connectionId: string, key: string, seconds: number) =>
    call<string>("redis_expire", { connectionId, key, seconds }),
  exportPdfText: (title: string, body: string, path: string) =>
    call<string>("export_pdf_text", { title, body, path }),
  convertBackup: (source: string, destination: string, operationId?: string) =>
    call<string>("convert_backup", { source, destination, operationId }),
  installSchedule: (intervalMinutes: number) => call<string>("install_schedule", { intervalMinutes }),
  mailSettings: () => call<MailSettings>("mail_settings"),
  saveMailSettings: (request: Record<string, unknown>) => call<MailSettings>("save_mail_settings", { request }),
};

export interface PlanNode {
  label: string;
  detail: string;
  cost?: number | null;
  rows?: number | null;
  children: PlanNode[];
}

export interface DebugReport {
  available: boolean;
  message: string;
  steps: string[];
}

export interface DebugCell {
  name: string;
  value: string;
  detail: string;
}

export interface DebugSnapshot {
  status: string;
  function: string;
  line?: number | null;
  message: string;
  variables: DebugCell[];
  stack: DebugCell[];
  targetNote: string;
}

export interface MailSettings {
  host: string;
  port: number;
  username: string;
  from: string;
  starttls: boolean;
  hasPassword: boolean;
}

export interface SyncStatement {
  id: string;
  kind: string;
  sql: string;
  warning: boolean;
}

export interface ForeignKeyRef {
  column: string;
  refSchema: string;
  refTable: string;
  refColumn: string;
}

export interface GridResult {
  columns: string[];
  rows: JsonValue[][];
  note: string;
}

export interface Snippet {
  id: string;
  name: string;
  engine: string;
  body: string;
  updatedAt: string;
}

export interface ObjectRef {
  connectionId: string;
  database: string;
  schema: string;
  table?: string;
}

export interface TableRef extends ObjectRef {
  table: string;
}

export interface FileTransferRequest extends TableRef {
  format: string;
  path: string;
}

/** The kind describes what was actually produced, including a partial data script. */
export interface FileExportResult extends ExportResult {
  kind: "table-export" | "native-backup" | "sqlite-script" | "data-only";
  note: string;
}

export interface PairRequest {
  sourceId: string;
  sourceDatabase: string;
  sourceSchema: string;
  sourceTable: string;
  targetId: string;
  targetDatabase: string;
  targetSchema: string;
  targetTable: string;
  apply?: boolean;
  deleteMissing?: boolean;
  allowDestructive?: boolean;
  selectedIds?: string[];
}

export interface BackupRequest {
  connectionId: string;
  database: string;
  schema: string;
  path: string;
  mode?: "auto" | "native" | "builtin";
}

export interface RestoreRequest {
  connectionId: string;
  database: string;
  path: string;
}

export interface ErGraph {
  tables: { name: string; columns: ColumnMeta[] }[];
  edges: { fromTable: string; fromColumn: string; toTable: string; toColumn: string }[];
  note: string;
}

export interface DiffResult {
  sql: string;
  notes: string[];
  applied: boolean;
  statements?: SyncStatement[];
}

export interface SyncResult {
  inserted: number;
  updated: number;
  deleted: number;
  truncated: boolean;
  note: string;
}

export interface AiSettings {
  endpoint: string;
  model: string;
  hasKey: boolean;
}

export interface AiSettingsRequest {
  endpoint: string;
  model: string;
  apiKey: string;
}

export interface AiAskRequest {
  task: string;
  sql: string;
  engine: string;
  schemaText: string;
  prompt: string;
  targetEngine?: string;
  model?: string;
}

export interface Job {
  id: string;
  name: string;
  kind: string;
  connectionId: string;
  database: string;
  schema: string;
  table: string;
  sql: string;
  path: string;
  intervalSecs: number;
  nextRunAt: number;
  enabled: boolean;
  lastMessage: string;
  notifyEmail?: string;
  profileIds?: string[];
}

export interface Dashboard {
  id: string;
  name: string;
  pages: JsonValue[];
}

export interface DataModel {
  id: string;
  name: string;
  level: string;
  note: string;
  entities: {
    name: string;
    attributes: { name: string; dataType: string; pk: boolean; nullable: boolean }[];
  }[];
  relations: {
    fromEntity: string;
    toEntity: string;
    fromAttr: string;
    toAttr: string;
    label: string;
  }[];
}

export interface Favorite {
  id: string;
  connectionId: string;
  database: string;
  schema: string;
  objectName: string;
  kind: string;
}

export async function pickSqliteFile(): Promise<string | null> {
  const selected = await open({
    title: "选择 SQLite 文件",
    multiple: false,
    directory: false,
    filters: [{ name: "SQLite", extensions: ["sqlite", "sqlite3", "db"] }],
  });
  return typeof selected === "string" ? selected : null;
}

export async function pickCsvFile(): Promise<string | null> {
  const selected = await open({
    title: "选择 CSV",
    multiple: false,
    directory: false,
    filters: [{ name: "CSV", extensions: ["csv", "txt"] }],
  });
  return typeof selected === "string" ? selected : null;
}

export async function pickSaveCsv(defaultPath: string): Promise<string | null> {
  const selected = await save({
    title: "导出 CSV",
    defaultPath,
    filters: [{ name: "CSV", extensions: ["csv"] }],
  });
  return selected ?? null;
}

export async function pickSavePath(defaultPath: string): Promise<string | null> {
  const selected = await save({ title: "保存文件", defaultPath });
  return selected ?? null;
}

export async function pickOpenPath(): Promise<string | null> {
  const selected = await open({ title: "选择文件", multiple: false, directory: false });
  return typeof selected === "string" ? selected : null;
}
