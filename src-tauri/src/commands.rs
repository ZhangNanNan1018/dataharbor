use tauri::State;

use crate::data::{
    commit_changes as commit_rows, export_sql_csv as export_sql, export_table_csv as export_table,
    fetch_rows as load_rows, import_table_csv as import_csv,
};
use crate::error::AppResult;
use crate::models::{
    AppInfo, ColumnMeta, CommitRequest, CommitResult, ConnectResult, ConnectionPublic, DataPage,
    ExecuteSqlRequest, ExecuteSqlResponse, ExportResult, ExportSqlRequest, ExportTableRequest,
    FetchRowsRequest, ImportCsvRequest, ImportResult, SaveConnectionRequest, SaveConnectionResult,
    TestResult,
};
use crate::query;
use crate::schema;
use crate::state::AppState;

#[tauri::command]
pub fn app_info(state: State<AppState>) -> AppInfo {
    state.app_info()
}

#[tauri::command]
pub fn list_connections(state: State<AppState>) -> Vec<ConnectionPublic> {
    state.list_connections()
}

#[tauri::command]
pub fn save_connection(
    state: State<AppState>,
    request: SaveConnectionRequest,
) -> AppResult<SaveConnectionResult> {
    state.save_connection(request)
}

#[tauri::command]
pub fn delete_connection(state: State<AppState>, id: String) -> AppResult<()> {
    state.delete_connection(&id)
}

#[tauri::command]
pub fn test_connection(
    state: State<AppState>,
    request: SaveConnectionRequest,
) -> AppResult<TestResult> {
    state.test_connection(request)
}

#[tauri::command]
pub fn connect(state: State<AppState>, id: String) -> AppResult<ConnectResult> {
    state.connect(&id)
}

#[tauri::command]
pub fn disconnect(state: State<AppState>, id: String) {
    state.disconnect(&id);
}

#[tauri::command]
pub fn create_sample_database(state: State<AppState>) -> AppResult<ConnectionPublic> {
    state.create_sample_database()
}

#[tauri::command]
pub fn list_databases(state: State<AppState>, connection_id: String) -> AppResult<Vec<String>> {
    schema::list_databases(&state, &connection_id)
}

#[tauri::command]
pub fn list_schemas(
    state: State<AppState>,
    connection_id: String,
    database: String,
) -> AppResult<Vec<String>> {
    schema::list_schemas(&state, &connection_id, &database)
}

#[tauri::command]
pub fn list_tables(
    state: State<AppState>,
    connection_id: String,
    database: String,
    schema: String,
) -> AppResult<Vec<String>> {
    schema::list_tables(&state, &connection_id, &database, &schema)
}

#[tauri::command]
pub fn list_views(
    state: State<AppState>,
    connection_id: String,
    database: String,
    schema: String,
) -> AppResult<Vec<String>> {
    schema::list_views(&state, &connection_id, &database, &schema)
}

#[tauri::command]
pub fn list_columns(
    state: State<AppState>,
    connection_id: String,
    database: String,
    schema: String,
    object_name: String,
) -> AppResult<Vec<ColumnMeta>> {
    schema::list_columns(&state, &connection_id, &database, &schema, &object_name)
}

#[tauri::command]
pub fn execute_sql(
    state: State<AppState>,
    request: ExecuteSqlRequest,
) -> AppResult<ExecuteSqlResponse> {
    query::execute_sql(&state, request)
}

#[tauri::command]
pub fn fetch_rows(state: State<AppState>, request: FetchRowsRequest) -> AppResult<DataPage> {
    load_rows(&state, request)
}

#[tauri::command]
pub fn commit_changes(state: State<AppState>, request: CommitRequest) -> AppResult<CommitResult> {
    commit_rows(&state, request)
}

#[tauri::command]
pub fn export_table_csv(
    state: State<AppState>,
    request: ExportTableRequest,
) -> AppResult<ExportResult> {
    export_table(&state, request)
}

#[tauri::command]
pub fn export_sql_csv(
    state: State<AppState>,
    request: ExportSqlRequest,
) -> AppResult<ExportResult> {
    export_sql(&state, request)
}

#[tauri::command]
pub fn import_table_csv(
    state: State<AppState>,
    request: ImportCsvRequest,
) -> AppResult<ImportResult> {
    import_csv(&state, request)
}

#[tauri::command]
pub fn format_sql(request: crate::studio::SqlText) -> String {
    crate::studio::format_sql(request)
}

#[tauri::command]
pub fn bind_sql(request: crate::studio::BindRequest) -> AppResult<String> {
    crate::studio::bind_sql(request)
}

#[tauri::command]
pub fn explain_query(state: State<AppState>, connection_id: String, database: String, sql: String) -> AppResult<crate::studio::GridResult> {
    crate::studio::explain_query(&state, &connection_id, &database, &sql)
}

#[tauri::command]
pub fn list_snippets(state: State<AppState>) -> AppResult<Vec<crate::studio::Snippet>> {
    crate::studio::list_snippets(&state)
}

#[tauri::command]
pub fn save_snippet(state: State<AppState>, snippet: crate::studio::Snippet) -> AppResult<crate::studio::Snippet> {
    crate::studio::save_snippet(&state, snippet)
}

#[tauri::command]
pub fn delete_snippet(state: State<AppState>, id: String) -> AppResult<()> {
    crate::studio::delete_snippet(&state, &id)
}

#[tauri::command]
pub fn er_diagram(state: State<AppState>, request: crate::studio::ObjectRef) -> AppResult<crate::studio::ErGraph> {
    crate::studio::er_diagram(&state, &request)
}

#[tauri::command]
pub fn export_table_file(state: State<AppState>, request: crate::studio::FileExportRequest) -> AppResult<crate::studio::FileExportResult> {
    crate::studio::export_file(&state, request)
}

#[tauri::command]
pub fn import_table_file(state: State<AppState>, request: crate::studio::FileImportRequest) -> AppResult<usize> {
    crate::studio::import_file(&state, request)
}

#[tauri::command]
pub fn diff_structure(state: State<AppState>, request: crate::studio::PairRequest) -> AppResult<crate::studio::DiffResult> {
    crate::studio::diff_structure(&state, &request)
}

#[tauri::command]
pub fn transfer_rows(state: State<AppState>, request: crate::studio::PairRequest) -> AppResult<crate::studio::SyncResult> {
    crate::studio::transfer_rows(&state, &request)
}

#[tauri::command]
pub fn sync_rows(state: State<AppState>, request: crate::studio::PairRequest) -> AppResult<crate::studio::SyncResult> {
    crate::studio::sync_rows(&state, &request)
}

#[tauri::command]
pub fn backup_sql(state: State<AppState>, request: crate::studio::BackupRequest) -> AppResult<crate::studio::FileExportResult> {
    crate::studio::backup_sql(&state, &request)
}

#[tauri::command]
pub fn restore_sql(state: State<AppState>, request: crate::studio::RestoreRequest) -> AppResult<String> {
    crate::studio::restore_sql(&state, &request)
}

#[tauri::command]
pub fn generate_rows(state: State<AppState>, request: crate::studio::GenerateRequest) -> AppResult<usize> {
    crate::studio::generate_rows(&state, &request)
}

#[tauri::command]
pub fn mask_columns(state: State<AppState>, request: crate::studio::MaskRequest) -> AppResult<String> {
    crate::studio::mask_columns(&state, &request)
}

#[tauri::command]
pub fn data_dictionary(state: State<AppState>, request: crate::studio::ObjectRef) -> AppResult<String> {
    crate::studio::data_dictionary(&state, &request)
}

#[tauri::command]
pub fn analyze_schema(state: State<AppState>, request: crate::studio::ObjectRef) -> AppResult<crate::studio::GridResult> {
    crate::studio::analyze_schema(&state, &request)
}

#[tauri::command]
pub fn list_accounts(state: State<AppState>, connection_id: String, database: String) -> AppResult<crate::studio::GridResult> {
    crate::studio::list_accounts(&state, &connection_id, &database)
}

#[tauri::command]
pub fn server_activity(state: State<AppState>, connection_id: String, database: String) -> AppResult<crate::studio::GridResult> {
    crate::studio::server_activity(&state, &connection_id, &database)
}

#[tauri::command]
pub fn preview_design(request: crate::studio::DesignRequest) -> AppResult<String> {
    crate::studio::preview_design(&request)
}

#[tauri::command]
pub fn ai_settings(state: State<AppState>) -> AppResult<crate::studio::AiSettingsPublic> {
    crate::studio::ai_settings(&state)
}

#[tauri::command]
pub fn save_ai_settings(state: State<AppState>, request: crate::studio::AiSettingsRequest) -> AppResult<crate::studio::AiSettingsPublic> {
    crate::studio::save_ai_settings(&state, request)
}

#[tauri::command]
pub fn ai_ask(state: State<AppState>, request: crate::studio::AiAskRequest) -> AppResult<String> {
    crate::studio::ai_ask(&state, request)
}

#[tauri::command]
pub fn list_jobs(state: State<AppState>) -> AppResult<Vec<crate::studio::Job>> {
    crate::studio::list_jobs(&state)
}

#[tauri::command]
pub fn save_job(state: State<AppState>, job: crate::studio::Job) -> AppResult<crate::studio::Job> {
    crate::studio::save_job(&state, job)
}

#[tauri::command]
pub fn delete_job(state: State<AppState>, id: String) -> AppResult<()> {
    crate::studio::delete_job(&state, &id)
}

#[tauri::command]
pub fn run_due_jobs(state: State<AppState>) -> AppResult<Vec<String>> {
    crate::studio::run_due_jobs(&state)
}

#[tauri::command]
pub fn list_dashboards(state: State<AppState>) -> AppResult<Vec<crate::studio::Dashboard>> {
    crate::studio::list_dashboards(&state)
}

#[tauri::command]
pub fn save_dashboard(state: State<AppState>, dashboard: crate::studio::Dashboard) -> AppResult<crate::studio::Dashboard> {
    crate::studio::save_dashboard(&state, dashboard)
}

#[tauri::command]
pub fn delete_dashboard(state: State<AppState>, id: String) -> AppResult<()> {
    crate::studio::delete_dashboard(&state, &id)
}

#[tauri::command]
pub fn list_models(state: State<AppState>) -> AppResult<Vec<crate::studio::DataModel>> {
    crate::studio::list_models(&state)
}

#[tauri::command]
pub fn save_model(state: State<AppState>, model: crate::studio::DataModel) -> AppResult<crate::studio::DataModel> {
    crate::studio::save_model(&state, model)
}

#[tauri::command]
pub fn delete_model(state: State<AppState>, id: String) -> AppResult<()> {
    crate::studio::delete_model(&state, &id)
}

#[tauri::command]
pub fn reverse_model(state: State<AppState>, request: crate::studio::ObjectRef, name: String) -> AppResult<crate::studio::DataModel> {
    crate::studio::reverse_model(&state, &request, &name)
}

#[tauri::command]
pub fn forward_sql(model: crate::studio::DataModel, engine: crate::models::Engine) -> AppResult<String> {
    crate::studio::forward_sql(&model, engine)
}

#[tauri::command]
pub fn model_html(model: crate::studio::DataModel) -> String {
    crate::studio::model_html(&model)
}

#[tauri::command]
pub fn list_favorites(state: State<AppState>) -> AppResult<Vec<crate::studio::Favorite>> {
    crate::studio::list_favorites(&state)
}

#[tauri::command]
pub fn save_favorite(state: State<AppState>, favorite: crate::studio::Favorite) -> AppResult<crate::studio::Favorite> {
    crate::studio::save_favorite(&state, favorite)
}

#[tauri::command]
pub fn delete_favorite(state: State<AppState>, id: String) -> AppResult<()> {
    crate::studio::delete_favorite(&state, &id)
}

#[tauri::command]
pub fn search_catalog(state: State<AppState>, request: crate::studio::ObjectRef, query: String) -> AppResult<crate::studio::GridResult> {
    crate::studio::search_catalog(&state, &request, &query)
}

#[tauri::command]
pub fn redis_publish(state: State<AppState>, connection_id: String, channel: String, message: String) -> AppResult<i64> {
    crate::studio::redis_publish(&state, &connection_id, &channel, &message)
}

#[tauri::command]
pub fn redis_listen(state: State<AppState>, connection_id: String, channel: String, wait_ms: u64) -> AppResult<Vec<String>> {
    crate::studio::redis_listen(&state, &connection_id, &channel, wait_ms)
}

#[tauri::command]
pub fn export_project(state: State<AppState>, path: String) -> AppResult<String> {
    crate::studio::export_project(&state, &path)
}

#[tauri::command]
pub fn import_project(state: State<AppState>, path: String) -> AppResult<String> {
    crate::studio::import_project(&state, &path)
}

#[tauri::command]
pub fn explain_plan(
    state: State<AppState>,
    connection_id: String,
    database: String,
    sql: String,
) -> AppResult<crate::plan::PlanNode> {
    crate::studio::explain_plan(&state, &connection_id, &database, &sql)
}

#[tauri::command]
pub fn foreign_keys(
    state: State<AppState>,
    connection_id: String,
    database: String,
    schema: String,
    table: String,
) -> AppResult<Vec<crate::studio::ForeignKeyRef>> {
    crate::studio::foreign_keys(&state, &connection_id, &database, &schema, &table)
}

#[tauri::command]
pub fn reference_rows(
    state: State<AppState>,
    connection_id: String,
    database: String,
    schema: String,
    table: String,
    column: String,
    query: String,
) -> AppResult<crate::studio::GridResult> {
    crate::studio::reference_rows(&state, &connection_id, &database, &schema, &table, &column, &query)
}

#[tauri::command]
pub fn debug_status(state: State<AppState>, connection_id: String, database: String) -> AppResult<crate::studio::DebugReport> {
    crate::studio::debug_status(&state, &connection_id, &database)
}

#[tauri::command]
pub fn debug_launch(
    state: State<AppState>,
    connection_id: String,
    database: String,
    function: String,
    call_sql: String,
    line: i32,
) -> AppResult<crate::pldbg::DebugSnapshot> {
    crate::pldbg::launch(&state, &connection_id, &database, &function, &call_sql, line)
}

#[tauri::command]
pub fn debug_step(state: State<AppState>, connection_id: String, action: String) -> AppResult<crate::pldbg::DebugSnapshot> {
    crate::pldbg::step(&state, &connection_id, &action)
}

#[tauri::command]
pub fn debug_stop(state: State<AppState>, connection_id: String) -> AppResult<crate::pldbg::DebugSnapshot> {
    crate::pldbg::stop(&state, &connection_id)
}

#[tauri::command]
pub fn profile_table(state: State<AppState>, request: crate::studio::ObjectRef) -> AppResult<crate::studio::GridResult> {
    crate::studio::profile_table(&state, &request)
}

#[tauri::command]
pub fn redis_expire(state: State<AppState>, connection_id: String, key: String, seconds: i64) -> AppResult<String> {
    crate::studio::redis_expire(&state, &connection_id, &key, seconds)
}

#[tauri::command]
pub fn export_pdf_text(title: String, body: String, path: String) -> AppResult<String> {
    crate::studio::export_pdf_text(&title, &body, &path)
}

#[tauri::command]
pub fn convert_backup(source: String, destination: String) -> AppResult<String> {
    crate::studio::convert_backup(&source, &destination)
}

#[tauri::command]
pub fn install_schedule(state: State<AppState>, interval_minutes: u64) -> AppResult<String> {
    crate::studio::install_schedule(&state, interval_minutes)
}

#[tauri::command]
pub fn mail_settings(state: State<AppState>) -> AppResult<crate::studio::MailSettings> {
    crate::studio::mail_settings(&state)
}

#[tauri::command]
pub fn save_mail_settings(state: State<AppState>, request: crate::studio::MailSave) -> AppResult<crate::studio::MailSettings> {
    crate::studio::save_mail_settings(&state, request)
}
