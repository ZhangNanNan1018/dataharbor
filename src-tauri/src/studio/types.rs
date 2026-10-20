use crate::catalogdiff;
use crate::models::{ColumnMeta, Engine};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SqlText {
    pub sql: String,
    #[serde(default)]
    pub minify: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParamBind {
    pub name: String,
    pub value: JsonValue,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindRequest {
    pub sql: String,
    pub engine: Engine,
    #[serde(default)]
    pub params: Vec<ParamBind>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snippet {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub engine: String,
    pub body: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GridResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<JsonValue>>,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErTable {
    pub name: String,
    pub columns: Vec<ColumnMeta>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErEdge {
    pub from_table: String,
    pub from_column: String,
    pub to_table: String,
    pub to_column: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErGraph {
    pub tables: Vec<ErTable>,
    pub edges: Vec<ErEdge>,
    pub note: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectRef {
    pub connection_id: String,
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub schema: String,
    #[serde(default)]
    pub table: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileExportRequest {
    pub connection_id: String,
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub schema: String,
    pub table: String,
    pub format: String,
    pub path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileExportResult {
    pub path: String,
    pub row_count: u64,
    pub truncated: bool,
    pub kind: String,
    pub note: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileImportRequest {
    pub connection_id: String,
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub schema: String,
    pub table: String,
    pub format: String,
    pub path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairRequest {
    pub source_id: String,
    #[serde(default)]
    pub source_database: String,
    #[serde(default)]
    pub source_schema: String,
    pub source_table: String,
    pub target_id: String,
    #[serde(default)]
    pub target_database: String,
    #[serde(default)]
    pub target_schema: String,
    pub target_table: String,
    #[serde(default)]
    pub apply: bool,
    #[serde(default)]
    pub delete_missing: bool,
    #[serde(default)]
    pub allow_destructive: bool,
    #[serde(default)]
    pub selected_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffResult {
    pub sql: String,
    pub notes: Vec<String>,
    pub applied: bool,
    pub statements: Vec<catalogdiff::SyncStatement>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncResult {
    pub inserted: usize,
    pub updated: usize,
    pub deleted: usize,
    pub truncated: bool,
    pub note: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupRequest {
    pub connection_id: String,
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub schema: String,
    pub path: String,
    /// auto, native, or builtin.
    #[serde(default)]
    pub mode: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreRequest {
    pub connection_id: String,
    #[serde(default)]
    pub database: String,
    pub path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateRequest {
    pub connection_id: String,
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub schema: String,
    pub table: String,
    pub count: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaskRequest {
    pub connection_id: String,
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub schema: String,
    pub table: String,
    pub columns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: String,
    #[serde(default)]
    pub revision: String,
    pub name: String,
    pub kind: String,
    pub connection_id: String,
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub schema: String,
    #[serde(default)]
    pub table: String,
    #[serde(default)]
    pub sql: String,
    #[serde(default)]
    pub path: String,
    #[serde(default = "default_interval")]
    pub interval_secs: u64,
    #[serde(default)]
    pub next_run_at: u64,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub last_message: String,
    #[serde(default)]
    pub notify_email: String,
    #[serde(default)]
    pub profile_ids: Vec<String>,
}

fn default_interval() -> u64 {
    3600
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dashboard {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub pages: Vec<JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelAttr {
    pub name: String,
    #[serde(default)]
    pub data_type: String,
    #[serde(default)]
    pub pk: bool,
    #[serde(default = "yes")]
    pub nullable: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelEntity {
    pub name: String,
    #[serde(default)]
    pub attributes: Vec<ModelAttr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRel {
    pub from_entity: String,
    pub to_entity: String,
    #[serde(default)]
    pub from_attr: String,
    #[serde(default)]
    pub to_attr: String,
    #[serde(default)]
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataModel {
    pub id: String,
    pub name: String,
    pub level: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub entities: Vec<ModelEntity>,
    #[serde(default)]
    pub relations: Vec<ModelRel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Favorite {
    pub id: String,
    pub connection_id: String,
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub schema: String,
    pub object_name: String,
    #[serde(default)]
    pub kind: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSettingsRequest {
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub clear_key: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSettingsPublic {
    pub endpoint: String,
    pub model: String,
    pub has_key: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiAskRequest {
    pub task: String,
    #[serde(default)]
    pub sql: String,
    #[serde(default)]
    pub engine: String,
    #[serde(default)]
    pub schema_text: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub target_engine: String,
    #[serde(default)]
    pub model: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignColumn {
    pub name: String,
    #[serde(default)]
    pub data_type: String,
    #[serde(default)]
    pub nullable: bool,
    #[serde(default)]
    pub primary_key: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignRequest {
    pub engine: Engine,
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub schema: String,
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub columns: Vec<DesignColumn>,
    #[serde(default)]
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailSettings {
    #[serde(default)]
    pub host: String,
    #[serde(default = "default_mail_port")]
    pub port: u16,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub starttls: bool,
    #[serde(default)]
    pub has_password: bool,
}

pub(super) fn default_mail_port() -> u16 {
    587
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailSave {
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub starttls: bool,
    #[serde(default)]
    pub clear_password: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugReport {
    pub available: bool,
    pub message: String,
    pub steps: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForeignKeyRef {
    pub column: String,
    pub ref_schema: String,
    pub ref_table: String,
    pub ref_column: String,
}
