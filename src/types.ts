export type Engine =
  | "mysql"
  | "mariadb"
  | "postgres"
  | "sqlite"
  | "mongodb"
  | "redis"
  | "mssql"
  | "oracle"
  | "snowflake"
  | "dameng";

export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };

export interface ConnectionPublic {
  id: string;
  name: string;
  group: string;
  color: string;
  engine: Engine;
  host: string;
  port: number;
  username: string;
  hasPassword: boolean;
  database: string;
  filePath: string;
  useSsl: boolean;
  sslInsecure: boolean;
  sshEnabled: boolean;
  sshHost: string;
  sshPort: number;
  sshUsername: string;
  hasSshPassword: boolean;
  sshKeyPath: string;
  httpTunnel: boolean;
  httpProxyHost: string;
  httpProxyPort: number;
  serverMode: string;
  authMethod: string;
  warehouse: string;
  schemaName: string;
  addressMap: string;
  connected: boolean;
  placeholder: boolean;
}

export interface SaveConnectionRequest {
  id?: string | null;
  name: string;
  group: string;
  color: string;
  engine: Engine;
  host: string;
  port: number;
  username: string;
  password?: string | null;
  clearPassword: boolean;
  database: string;
  filePath: string;
  useSsl: boolean;
  sslInsecure: boolean;
  sshEnabled: boolean;
  sshHost: string;
  sshPort: number;
  sshUsername: string;
  sshPassword?: string | null;
  clearSshPassword: boolean;
  sshKeyPath: string;
  httpTunnel: boolean;
  httpProxyHost: string;
  httpProxyPort: number;
  serverMode: string;
  authMethod: string;
  warehouse: string;
  schemaName: string;
  addressMap: string;
}

export interface SaveConnectionResult {
  connection: ConnectionPublic;
  needsReconnect: boolean;
}

export interface TestResult {
  ok: boolean;
  message: string;
  version?: string | null;
}

export interface ConnectResult {
  id: string;
  engine: Engine;
  version: string;
  database: string;
}

export interface ColumnMeta {
  name: string;
  dataType: string;
  nullable: boolean;
  primaryKey: boolean;
  autoIncrement: boolean;
  defaultValue?: string | null;
}

export interface StatementResult {
  index: number;
  sql: string;
  ok: boolean;
  error?: string | null;
  columns: ColumnMeta[];
  rows: JsonValue[][];
  rowsAffected: number;
  totalRows?: number | null;
  page: number;
  pageSize: number;
  pageable: boolean;
  elapsedMs: number;
  message: string;
  truncated: boolean;
  hasMore: boolean;
}

export interface ExecuteSqlResponse {
  statements: StatementResult[];
  stopped: boolean;
}

export interface FilterSpec {
  column: string;
  op: string;
  value?: string | null;
}

export interface SortSpec {
  column: string;
  desc: boolean;
}

export interface FetchRowsRequest {
  connectionId: string;
  database: string;
  schema: string;
  table: string;
  filters: FilterSpec[];
  sorts: SortSpec[];
  page: number;
  pageSize: number;
}

export interface DataPage {
  columns: ColumnMeta[];
  rows: JsonValue[][];
  originalRows?: FieldValue[][] | null;
  total: number;
  page: number;
  pageSize: number;
  readOnly: boolean;
}

export interface FieldValue {
  column: string;
  value: JsonValue;
}

export interface RowChange {
  kind: "insert" | "update" | "delete";
  pk: FieldValue[];
  values: FieldValue[];
  original?: FieldValue[];
}

export interface CommitResult {
  applied: number;
}

export interface ExportResult {
  path: string;
  rowCount: number;
  truncated: boolean;
}

export interface ImportResult {
  inserted: number;
}

export interface AppInfo {
  name: string;
  version: string;
  encryption: string;
  configDir: string;
  independent: boolean;
}

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
