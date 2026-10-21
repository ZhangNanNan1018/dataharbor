use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Mysql,
    Mariadb,
    Postgres,
    Sqlite,
    Mongodb,
    Redis,
    Mssql,
    Oracle,
    Snowflake,
    Dameng,
}

impl Engine {
    pub fn is_sql(self) -> bool {
        matches!(
            self,
            Engine::Mysql
                | Engine::Mariadb
                | Engine::Postgres
                | Engine::Sqlite
                | Engine::Mssql
                | Engine::Oracle
                | Engine::Snowflake
                | Engine::Dameng
        )
    }

    pub fn default_port(self) -> u16 {
        match self {
            Engine::Mysql | Engine::Mariadb => 3306,
            Engine::Postgres => 5432,
            Engine::Mongodb => 27017,
            Engine::Redis => 6379,
            Engine::Mssql => 1433,
            Engine::Oracle => 1521,
            Engine::Snowflake => 443,
            Engine::Dameng => 5236,
            Engine::Sqlite => 0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Engine::Mysql => "MySQL",
            Engine::Mariadb => "MariaDB",
            Engine::Postgres => "PostgreSQL",
            Engine::Sqlite => "SQLite",
            Engine::Mongodb => "MongoDB",
            Engine::Redis => "Redis",
            Engine::Mssql => "SQL Server",
            Engine::Oracle => "Oracle",
            Engine::Snowflake => "Snowflake",
            Engine::Dameng => "达梦",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnMeta {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub primary_key: bool,
    pub auto_increment: bool,
    #[serde(default)]
    pub default_value: Option<String>,
}

impl ColumnMeta {
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            data_type: String::new(),
            nullable: true,
            primary_key: false,
            auto_increment: false,
            default_value: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryOutcome {
    pub original_rows: Option<Vec<Vec<FieldValue>>>,
    pub columns: Vec<ColumnMeta>,
    pub rows: Vec<Vec<JsonValue>>,
    pub rows_affected: u64,
    pub truncated: bool,
}

impl QueryOutcome {
    pub fn empty() -> Self {
        Self {
            original_rows: None,
            columns: Vec::new(),
            rows: Vec::new(),
            rows_affected: 0,
            truncated: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredProfile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub group: String,
    #[serde(default = "default_color")]
    pub color: String,
    pub engine: Engine,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password_enc: String,
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub file_path: String,
    #[serde(default)]
    pub use_ssl: bool,
    #[serde(default)]
    pub ssl_insecure: bool,
    #[serde(default)]
    pub ssh_enabled: bool,
    #[serde(default)]
    pub ssh_host: String,
    #[serde(default = "default_ssh_port")]
    pub ssh_port: u16,
    #[serde(default)]
    pub ssh_username: String,
    #[serde(default)]
    pub ssh_password_enc: String,
    #[serde(default)]
    pub ssh_key_path: String,
    #[serde(default)]
    pub http_tunnel: bool,
    #[serde(default)]
    pub http_proxy_host: String,
    #[serde(default = "default_http_port")]
    pub http_proxy_port: u16,
    /// standalone, cluster, or sentinel. Used by Redis.
    #[serde(default)]
    pub server_mode: String,
    /// password, cleartext, ldap, pam, gssapi, or windows.
    #[serde(default)]
    pub auth_method: String,
    /// Snowflake warehouse. Empty for other engines.
    #[serde(default)]
    pub warehouse: String,
    /// Snowflake schema. Empty means PUBLIC.
    #[serde(default)]
    pub schema_name: String,
    /// Redis Cluster advertised address map: `10.0.0.1:6379=127.0.0.1:40001`.
    #[serde(default)]
    pub address_map: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

fn default_color() -> String {
    "#6366f1".into()
}

fn default_ssh_port() -> u16 {
    22
}

fn default_http_port() -> u16 {
    8080
}

#[derive(Clone)]
pub struct SessionConfig {
    pub engine: Engine,
    pub host: String,
    pub port: u16,
    /// Certificate identity before a tunnel rewrites the TCP destination.
    /// Runtime-only: never accept this override from imported profiles.
    pub tls_server_name: Option<String>,
    pub username: String,
    pub password: String,
    pub database: String,
    pub file_path: String,
    pub use_ssl: bool,
    pub ssl_insecure: bool,
    pub ssh_password: String,
    pub server_mode: String,
    pub auth_method: String,
    pub warehouse: String,
    pub schema_name: String,
    pub address_map: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionPublic {
    pub id: String,
    pub name: String,
    pub group: String,
    pub color: String,
    pub engine: Engine,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub has_password: bool,
    pub database: String,
    pub file_path: String,
    pub use_ssl: bool,
    pub ssl_insecure: bool,
    pub ssh_enabled: bool,
    pub ssh_host: String,
    pub ssh_port: u16,
    pub ssh_username: String,
    pub has_ssh_password: bool,
    pub ssh_key_path: String,
    pub http_tunnel: bool,
    pub http_proxy_host: String,
    pub http_proxy_port: u16,
    pub server_mode: String,
    pub auth_method: String,
    pub warehouse: String,
    pub schema_name: String,
    pub address_map: String,
    pub connected: bool,
    pub placeholder: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveConnectionRequest {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub color: String,
    pub engine: Engine,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub clear_password: bool,
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub file_path: String,
    #[serde(default)]
    pub use_ssl: bool,
    #[serde(default)]
    pub ssl_insecure: bool,
    #[serde(default)]
    pub ssh_enabled: bool,
    #[serde(default)]
    pub ssh_host: String,
    #[serde(default)]
    pub ssh_port: u16,
    #[serde(default)]
    pub ssh_username: String,
    #[serde(default)]
    pub ssh_password: Option<String>,
    #[serde(default)]
    pub clear_ssh_password: bool,
    #[serde(default)]
    pub ssh_key_path: String,
    #[serde(default)]
    pub http_tunnel: bool,
    #[serde(default)]
    pub http_proxy_host: String,
    #[serde(default)]
    pub http_proxy_port: u16,
    #[serde(default)]
    pub server_mode: String,
    #[serde(default)]
    pub auth_method: String,
    #[serde(default)]
    pub warehouse: String,
    #[serde(default)]
    pub schema_name: String,
    #[serde(default)]
    pub address_map: String,
}

impl Default for SaveConnectionRequest {
    fn default() -> Self {
        Self {
            id: None,
            name: String::new(),
            group: String::new(),
            color: String::new(),
            engine: Engine::Sqlite,
            host: String::new(),
            port: 0,
            username: String::new(),
            password: None,
            clear_password: false,
            database: String::new(),
            file_path: String::new(),
            use_ssl: false,
            ssl_insecure: false,
            ssh_enabled: false,
            ssh_host: String::new(),
            ssh_port: 22,
            ssh_username: String::new(),
            ssh_password: None,
            clear_ssh_password: false,
            ssh_key_path: String::new(),
            http_tunnel: false,
            http_proxy_host: String::new(),
            http_proxy_port: 8080,
            server_mode: String::new(),
            auth_method: String::new(),
            warehouse: String::new(),
            schema_name: String::new(),
            address_map: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveConnectionResult {
    pub connection: ConnectionPublic,
    pub needs_reconnect: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    pub ok: bool,
    pub message: String,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectResult {
    pub id: String,
    pub engine: Engine,
    pub version: String,
    pub database: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteSqlRequest {
    pub connection_id: String,
    #[serde(default)]
    pub database: String,
    pub sql: String,
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_page_size")]
    pub page_size: u32,
    #[serde(default)]
    pub only_statement: Option<usize>,
}

pub fn default_page() -> u32 {
    1
}

pub fn default_page_size() -> u32 {
    100
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementResult {
    pub index: usize,
    pub sql: String,
    pub ok: bool,
    pub error: Option<String>,
    pub columns: Vec<ColumnMeta>,
    pub rows: Vec<Vec<JsonValue>>,
    pub rows_affected: u64,
    pub total_rows: Option<u64>,
    pub page: u32,
    pub page_size: u32,
    pub pageable: bool,
    pub elapsed_ms: u64,
    pub message: String,
    pub truncated: bool,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteSqlResponse {
    pub statements: Vec<StatementResult>,
    pub stopped: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterSpec {
    pub column: String,
    pub op: String,
    #[serde(default)]
    pub value: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SortSpec {
    pub column: String,
    #[serde(default)]
    pub desc: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchRowsRequest {
    pub connection_id: String,
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub schema: String,
    pub table: String,
    #[serde(default)]
    pub filters: Vec<FilterSpec>,
    #[serde(default)]
    pub sorts: Vec<SortSpec>,
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_page_size")]
    pub page_size: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DataPage {
    pub original_rows: Option<Vec<Vec<FieldValue>>>,
    pub columns: Vec<ColumnMeta>,
    pub rows: Vec<Vec<JsonValue>>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldValue {
    pub column: String,
    pub value: JsonValue,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RowChange {
    pub kind: String,
    /// Values observed by the editor. Used as an optimistic concurrency guard.
    #[serde(default)]
    pub original: Option<Vec<FieldValue>>,
    #[serde(default)]
    pub pk: Vec<FieldValue>,
    #[serde(default)]
    pub values: Vec<FieldValue>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitRequest {
    pub connection_id: String,
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub schema: String,
    pub table: String,
    pub changes: Vec<RowChange>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitResult {
    pub applied: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportTableRequest {
    pub connection_id: String,
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub schema: String,
    pub table: String,
    #[serde(default)]
    pub filters: Vec<FilterSpec>,
    #[serde(default)]
    pub sorts: Vec<SortSpec>,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportSqlRequest {
    pub connection_id: String,
    #[serde(default)]
    pub database: String,
    pub sql: String,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportCsvRequest {
    pub connection_id: String,
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub schema: String,
    pub table: String,
    pub path: String,
    #[serde(default)]
    pub truncate: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub path: String,
    pub row_count: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub inserted: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub encryption: String,
    pub config_dir: String,
    pub independent: bool,
}
