use tauri::Manager;

use crate::data::{commit_changes as commit_rows, fetch_rows as load_rows};
use crate::error::AppResult;
use crate::models::{
    AppInfo, ColumnMeta, CommitRequest, CommitResult, ConnectResult, ConnectionPublic, DataPage,
    ExecuteSqlRequest, ExecuteSqlResponse, ExportResult, ExportSqlRequest, ExportTableRequest,
    FetchRowsRequest, ImportCsvRequest, ImportResult, SaveConnectionRequest, SaveConnectionResult,
    TestResult,
};
use crate::operations::{OperationContext, OperationSnapshot};
use crate::query;
use crate::schema;
use crate::state::AppState;

// Blocking drivers and native tools run outside the window and async workers.
async fn run_blocking<T: Send + 'static>(
    operation: impl FnOnce() -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    tauri::async_runtime::spawn_blocking(operation)
        .await
        .map_err(|error| crate::error::AppError::msg(format!("后台操作异常：{error}")))?
}

async fn run_operation<T: Send + 'static>(
    app: tauri::AppHandle,
    operation_id: Option<String>,
    action: impl FnOnce(&AppState, &OperationContext) -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    let operation = match operation_id {
        Some(id) => app.state::<AppState>().operations.claim(&id)?,
        None => OperationContext::detached(),
    };
    let worker = operation.clone();
    let result = run_blocking(move || {
        worker.checkpoint()?;
        action(&app.state::<AppState>(), &worker)
    })
    .await;
    operation.finish(&result);
    result
}

#[tauri::command]
pub fn create_operation(app: tauri::AppHandle, label: String) -> OperationSnapshot {
    app.state::<AppState>().operations.create(label)
}

#[tauri::command]
pub fn operation_status(app: tauri::AppHandle, id: String) -> AppResult<OperationSnapshot> {
    Ok(app.state::<AppState>().operations.get(&id)?.snapshot())
}

#[tauri::command]
pub async fn cancel_operation(app: tauri::AppHandle, id: String) -> AppResult<OperationSnapshot> {
    let operation = app.state::<AppState>().operations.get(&id)?;
    run_blocking(move || operation.request_cancel()).await
}

#[tauri::command]
pub fn release_operation(app: tauri::AppHandle, id: String) -> AppResult<()> {
    app.state::<AppState>().operations.release(&id)
}

#[tauri::command]
pub async fn app_info(app: tauri::AppHandle) -> AppResult<AppInfo> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        Ok(state.app_info())
    })
    .await
}

#[tauri::command]
pub async fn list_connections(app: tauri::AppHandle) -> AppResult<Vec<ConnectionPublic>> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        Ok(state.list_connections())
    })
    .await
}

#[tauri::command]
pub async fn save_connection(
    app: tauri::AppHandle,
    request: SaveConnectionRequest,
) -> AppResult<SaveConnectionResult> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        state.save_connection(request)
    })
    .await
}

#[tauri::command]
pub async fn delete_connection(app: tauri::AppHandle, id: String) -> AppResult<()> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        state.delete_connection(&id)
    })
    .await
}

#[tauri::command]
pub async fn test_connection(
    app: tauri::AppHandle,
    request: SaveConnectionRequest,
) -> AppResult<TestResult> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        state.test_connection(request)
    })
    .await
}

#[tauri::command]
pub async fn connect(app: tauri::AppHandle, id: String) -> AppResult<ConnectResult> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        state.connect(&id)
    })
    .await
}

#[tauri::command]
pub async fn disconnect(app: tauri::AppHandle, id: String) -> AppResult<()> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        Ok({
            state.disconnect(&id);
        })
    })
    .await
}

#[tauri::command]
pub async fn create_sample_database(app: tauri::AppHandle) -> AppResult<ConnectionPublic> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        state.create_sample_database()
    })
    .await
}

#[tauri::command]
pub async fn list_databases(
    app: tauri::AppHandle,
    connection_id: String,
) -> AppResult<Vec<String>> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        schema::list_databases(&state, &connection_id)
    })
    .await
}

#[tauri::command]
pub async fn list_schemas(
    app: tauri::AppHandle,
    connection_id: String,
    database: String,
) -> AppResult<Vec<String>> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        schema::list_schemas(&state, &connection_id, &database)
    })
    .await
}

#[tauri::command]
pub async fn list_tables(
    app: tauri::AppHandle,
    connection_id: String,
    database: String,
    schema: String,
) -> AppResult<Vec<String>> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        schema::list_tables(&state, &connection_id, &database, &schema)
    })
    .await
}

#[tauri::command]
pub async fn list_views(
    app: tauri::AppHandle,
    connection_id: String,
    database: String,
    schema: String,
) -> AppResult<Vec<String>> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        schema::list_views(&state, &connection_id, &database, &schema)
    })
    .await
}

#[tauri::command]
pub async fn list_columns(
    app: tauri::AppHandle,
    connection_id: String,
    database: String,
    schema: String,
    object_name: String,
) -> AppResult<Vec<ColumnMeta>> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        schema::list_columns(&state, &connection_id, &database, &schema, &object_name)
    })
    .await
}

#[tauri::command]
pub async fn execute_sql(
    app: tauri::AppHandle,
    request: ExecuteSqlRequest,
    operation_id: Option<String>,
) -> AppResult<ExecuteSqlResponse> {
    run_operation(app, operation_id, move |state, operation| {
        query::execute_sql_with_operation(state, request, operation)
    })
    .await
}

#[tauri::command]
pub async fn fetch_rows(app: tauri::AppHandle, request: FetchRowsRequest) -> AppResult<DataPage> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        load_rows(&state, request)
    })
    .await
}

#[tauri::command]
pub async fn commit_changes(
    app: tauri::AppHandle,
    request: CommitRequest,
) -> AppResult<CommitResult> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        commit_rows(&state, request)
    })
    .await
}

#[tauri::command]
pub async fn export_table_csv(
    app: tauri::AppHandle,
    request: ExportTableRequest,
    operation_id: Option<String>,
) -> AppResult<ExportResult> {
    run_operation(app, operation_id, move |state, operation| {
        crate::data::export_table_csv_with_operation(state, request, operation)
    })
    .await
}

#[tauri::command]
pub async fn export_sql_csv(
    app: tauri::AppHandle,
    request: ExportSqlRequest,
    operation_id: Option<String>,
) -> AppResult<ExportResult> {
    run_operation(app, operation_id, move |state, operation| {
        crate::data::export_sql_csv_with_operation(state, request, operation)
    })
    .await
}

#[tauri::command]
pub async fn import_table_csv(
    app: tauri::AppHandle,
    request: ImportCsvRequest,
    operation_id: Option<String>,
) -> AppResult<ImportResult> {
    run_operation(app, operation_id, move |state, operation| {
        crate::data::import_table_csv_with_operation(state, request, operation)
    })
    .await
}

#[tauri::command]
pub async fn format_sql(request: crate::studio::SqlText) -> AppResult<String> {
    run_blocking(move || Ok(crate::studio::format_sql(request))).await
}

#[tauri::command]
pub async fn bind_sql(request: crate::studio::BindRequest) -> AppResult<String> {
    run_blocking(move || crate::studio::bind_sql(request)).await
}

#[tauri::command]
pub async fn explain_query(
    app: tauri::AppHandle,
    connection_id: String,
    database: String,
    sql: String,
) -> AppResult<crate::studio::GridResult> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::explain_query(&state, &connection_id, &database, &sql)
    })
    .await
}

#[tauri::command]
pub async fn list_snippets(app: tauri::AppHandle) -> AppResult<Vec<crate::studio::Snippet>> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::list_snippets(&state)
    })
    .await
}

#[tauri::command]
pub async fn save_snippet(
    app: tauri::AppHandle,
    snippet: crate::studio::Snippet,
) -> AppResult<crate::studio::Snippet> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::save_snippet(&state, snippet)
    })
    .await
}

#[tauri::command]
pub async fn delete_snippet(app: tauri::AppHandle, id: String) -> AppResult<()> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::delete_snippet(&state, &id)
    })
    .await
}

#[tauri::command]
pub async fn er_diagram(
    app: tauri::AppHandle,
    request: crate::studio::ObjectRef,
) -> AppResult<crate::studio::ErGraph> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::er_diagram(&state, &request)
    })
    .await
}

#[tauri::command]
pub async fn export_table_file(
    app: tauri::AppHandle,
    request: crate::studio::FileExportRequest,
    operation_id: Option<String>,
) -> AppResult<crate::studio::FileExportResult> {
    run_operation(app, operation_id, move |state, operation| {
        crate::studio::export_file_with_operation(state, request, operation)
    })
    .await
}

#[tauri::command]
pub async fn import_table_file(
    app: tauri::AppHandle,
    request: crate::studio::FileImportRequest,
    operation_id: Option<String>,
) -> AppResult<usize> {
    run_operation(app, operation_id, move |state, operation| {
        crate::studio::import_file_with_operation(state, request, operation)
    })
    .await
}

#[tauri::command]
pub async fn diff_structure(
    app: tauri::AppHandle,
    request: crate::studio::PairRequest,
) -> AppResult<crate::studio::DiffResult> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::diff_structure(&state, &request)
    })
    .await
}

#[tauri::command]
pub async fn transfer_rows(
    app: tauri::AppHandle,
    request: crate::studio::PairRequest,
    operation_id: Option<String>,
) -> AppResult<crate::studio::SyncResult> {
    run_operation(app, operation_id, move |state, operation| {
        crate::studio::transfer_rows_with_operation(state, &request, operation)
    })
    .await
}

#[tauri::command]
pub async fn sync_rows(
    app: tauri::AppHandle,
    request: crate::studio::PairRequest,
    operation_id: Option<String>,
) -> AppResult<crate::studio::SyncResult> {
    run_operation(app, operation_id, move |state, operation| {
        crate::studio::sync_rows_with_operation(state, &request, operation)
    })
    .await
}

#[tauri::command]
pub async fn backup_sql(
    app: tauri::AppHandle,
    request: crate::studio::BackupRequest,
    operation_id: Option<String>,
) -> AppResult<crate::studio::FileExportResult> {
    run_operation(app, operation_id, move |state, operation| {
        crate::studio::backup_sql_with_operation(state, &request, operation)
    })
    .await
}

#[tauri::command]
pub async fn restore_sql(
    app: tauri::AppHandle,
    request: crate::studio::RestoreRequest,
    operation_id: Option<String>,
) -> AppResult<String> {
    run_operation(app, operation_id, move |state, operation| {
        crate::studio::restore_sql_with_operation(state, &request, operation)
    })
    .await
}

#[tauri::command]
pub async fn generate_rows(
    app: tauri::AppHandle,
    request: crate::studio::GenerateRequest,
) -> AppResult<usize> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::generate_rows(&state, &request)
    })
    .await
}

#[tauri::command]
pub async fn mask_columns(
    app: tauri::AppHandle,
    request: crate::studio::MaskRequest,
) -> AppResult<String> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::mask_columns(&state, &request)
    })
    .await
}

#[tauri::command]
pub async fn data_dictionary(
    app: tauri::AppHandle,
    request: crate::studio::ObjectRef,
) -> AppResult<String> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::data_dictionary(&state, &request)
    })
    .await
}

#[tauri::command]
pub async fn analyze_schema(
    app: tauri::AppHandle,
    request: crate::studio::ObjectRef,
) -> AppResult<crate::studio::GridResult> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::analyze_schema(&state, &request)
    })
    .await
}

#[tauri::command]
pub async fn list_accounts(
    app: tauri::AppHandle,
    connection_id: String,
    database: String,
) -> AppResult<crate::studio::GridResult> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::list_accounts(&state, &connection_id, &database)
    })
    .await
}

#[tauri::command]
pub async fn server_activity(
    app: tauri::AppHandle,
    connection_id: String,
    database: String,
) -> AppResult<crate::studio::GridResult> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::server_activity(&state, &connection_id, &database)
    })
    .await
}

#[tauri::command]
pub async fn preview_design(request: crate::studio::DesignRequest) -> AppResult<String> {
    run_blocking(move || crate::studio::preview_design(&request)).await
}

#[tauri::command]
pub async fn ai_settings(app: tauri::AppHandle) -> AppResult<crate::studio::AiSettingsPublic> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::ai_settings(&state)
    })
    .await
}

#[tauri::command]
pub async fn save_ai_settings(
    app: tauri::AppHandle,
    request: crate::studio::AiSettingsRequest,
) -> AppResult<crate::studio::AiSettingsPublic> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::save_ai_settings(&state, request)
    })
    .await
}

#[tauri::command]
pub async fn ai_ask(
    app: tauri::AppHandle,
    request: crate::studio::AiAskRequest,
) -> AppResult<String> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::ai_ask(&state, request)
    })
    .await
}

#[tauri::command]
pub async fn list_jobs(app: tauri::AppHandle) -> AppResult<Vec<crate::studio::Job>> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::list_jobs(&state)
    })
    .await
}

#[tauri::command]
pub async fn save_job(
    app: tauri::AppHandle,
    job: crate::studio::Job,
) -> AppResult<crate::studio::Job> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::save_job(&state, job)
    })
    .await
}

#[tauri::command]
pub async fn delete_job(app: tauri::AppHandle, id: String) -> AppResult<()> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::delete_job(&state, &id)
    })
    .await
}

#[tauri::command]
pub async fn run_due_jobs(app: tauri::AppHandle) -> AppResult<Vec<String>> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::run_due_jobs(&state)
    })
    .await
}

#[tauri::command]
pub async fn list_dashboards(app: tauri::AppHandle) -> AppResult<Vec<crate::studio::Dashboard>> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::list_dashboards(&state)
    })
    .await
}

#[tauri::command]
pub async fn save_dashboard(
    app: tauri::AppHandle,
    dashboard: crate::studio::Dashboard,
) -> AppResult<crate::studio::Dashboard> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::save_dashboard(&state, dashboard)
    })
    .await
}

#[tauri::command]
pub async fn delete_dashboard(app: tauri::AppHandle, id: String) -> AppResult<()> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::delete_dashboard(&state, &id)
    })
    .await
}

#[tauri::command]
pub async fn list_models(app: tauri::AppHandle) -> AppResult<Vec<crate::studio::DataModel>> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::list_models(&state)
    })
    .await
}

#[tauri::command]
pub async fn save_model(
    app: tauri::AppHandle,
    model: crate::studio::DataModel,
) -> AppResult<crate::studio::DataModel> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::save_model(&state, model)
    })
    .await
}

#[tauri::command]
pub async fn delete_model(app: tauri::AppHandle, id: String) -> AppResult<()> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::delete_model(&state, &id)
    })
    .await
}

#[tauri::command]
pub async fn reverse_model(
    app: tauri::AppHandle,
    request: crate::studio::ObjectRef,
    name: String,
) -> AppResult<crate::studio::DataModel> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::reverse_model(&state, &request, &name)
    })
    .await
}

#[tauri::command]
pub async fn forward_sql(
    model: crate::studio::DataModel,
    engine: crate::models::Engine,
) -> AppResult<String> {
    run_blocking(move || crate::studio::forward_sql(&model, engine)).await
}

#[tauri::command]
pub async fn model_html(model: crate::studio::DataModel) -> AppResult<String> {
    run_blocking(move || Ok(crate::studio::model_html(&model))).await
}

#[tauri::command]
pub async fn list_favorites(app: tauri::AppHandle) -> AppResult<Vec<crate::studio::Favorite>> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::list_favorites(&state)
    })
    .await
}

#[tauri::command]
pub async fn save_favorite(
    app: tauri::AppHandle,
    favorite: crate::studio::Favorite,
) -> AppResult<crate::studio::Favorite> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::save_favorite(&state, favorite)
    })
    .await
}

#[tauri::command]
pub async fn delete_favorite(app: tauri::AppHandle, id: String) -> AppResult<()> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::delete_favorite(&state, &id)
    })
    .await
}

#[tauri::command]
pub async fn search_catalog(
    app: tauri::AppHandle,
    request: crate::studio::ObjectRef,
    query: String,
) -> AppResult<crate::studio::GridResult> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::search_catalog(&state, &request, &query)
    })
    .await
}

#[tauri::command]
pub async fn redis_publish(
    app: tauri::AppHandle,
    connection_id: String,
    channel: String,
    message: String,
) -> AppResult<i64> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::redis_publish(&state, &connection_id, &channel, &message)
    })
    .await
}

#[tauri::command]
pub async fn redis_listen(
    app: tauri::AppHandle,
    connection_id: String,
    channel: String,
    wait_ms: u64,
) -> AppResult<Vec<String>> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::redis_listen(&state, &connection_id, &channel, wait_ms)
    })
    .await
}

#[tauri::command]
pub async fn export_project(app: tauri::AppHandle, path: String) -> AppResult<String> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::export_project(&state, &path)
    })
    .await
}

#[tauri::command]
pub async fn import_project(app: tauri::AppHandle, path: String) -> AppResult<String> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::import_project(&state, &path)
    })
    .await
}

#[tauri::command]
pub async fn explain_plan(
    app: tauri::AppHandle,
    connection_id: String,
    database: String,
    sql: String,
) -> AppResult<crate::plan::PlanNode> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::explain_plan(&state, &connection_id, &database, &sql)
    })
    .await
}

#[tauri::command]
pub async fn foreign_keys(
    app: tauri::AppHandle,
    connection_id: String,
    database: String,
    schema: String,
    table: String,
) -> AppResult<Vec<crate::studio::ForeignKeyRef>> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::foreign_keys(&state, &connection_id, &database, &schema, &table)
    })
    .await
}

#[tauri::command]
pub async fn reference_rows(
    app: tauri::AppHandle,
    connection_id: String,
    database: String,
    schema: String,
    table: String,
    column: String,
    query: String,
) -> AppResult<crate::studio::GridResult> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::reference_rows(
            &state,
            &connection_id,
            &database,
            &schema,
            &table,
            &column,
            &query,
        )
    })
    .await
}

#[tauri::command]
pub async fn debug_status(
    app: tauri::AppHandle,
    connection_id: String,
    database: String,
) -> AppResult<crate::studio::DebugReport> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::debug_status(&state, &connection_id, &database)
    })
    .await
}

#[tauri::command]
pub async fn debug_launch(
    app: tauri::AppHandle,
    connection_id: String,
    database: String,
    function: String,
    call_sql: String,
    line: i32,
) -> AppResult<crate::pldbg::DebugSnapshot> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::pldbg::launch(
            &state,
            &connection_id,
            &database,
            &function,
            &call_sql,
            line,
        )
    })
    .await
}

#[tauri::command]
pub async fn debug_step(
    app: tauri::AppHandle,
    connection_id: String,
    action: String,
) -> AppResult<crate::pldbg::DebugSnapshot> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::pldbg::step(&state, &connection_id, &action)
    })
    .await
}

#[tauri::command]
pub async fn debug_stop(
    app: tauri::AppHandle,
    connection_id: String,
) -> AppResult<crate::pldbg::DebugSnapshot> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::pldbg::stop(&state, &connection_id)
    })
    .await
}

#[tauri::command]
pub async fn profile_table(
    app: tauri::AppHandle,
    request: crate::studio::ObjectRef,
) -> AppResult<crate::studio::GridResult> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::profile_table(&state, &request)
    })
    .await
}

#[tauri::command]
pub async fn redis_expire(
    app: tauri::AppHandle,
    connection_id: String,
    key: String,
    seconds: i64,
) -> AppResult<String> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::redis_expire(&state, &connection_id, &key, seconds)
    })
    .await
}

#[tauri::command]
pub async fn export_pdf_text(title: String, body: String, path: String) -> AppResult<String> {
    run_blocking(move || crate::studio::export_pdf_text(&title, &body, &path)).await
}

#[tauri::command]
pub async fn convert_backup(
    app: tauri::AppHandle,
    source: String,
    destination: String,
    operation_id: Option<String>,
) -> AppResult<String> {
    run_operation(app, operation_id, move |_, operation| {
        crate::studio::convert_backup_with_operation(&source, &destination, operation)
    })
    .await
}

#[tauri::command]
pub async fn install_schedule(app: tauri::AppHandle, interval_minutes: u64) -> AppResult<String> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::install_schedule(&state, interval_minutes)
    })
    .await
}

#[tauri::command]
pub async fn mail_settings(app: tauri::AppHandle) -> AppResult<crate::studio::MailSettings> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::mail_settings(&state)
    })
    .await
}

#[tauri::command]
pub async fn save_mail_settings(
    app: tauri::AppHandle,
    request: crate::studio::MailSave,
) -> AppResult<crate::studio::MailSettings> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        crate::studio::save_mail_settings(&state, request)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn blocking_work_does_not_block_the_async_caller_and_errors_propagate() {
        let caller = std::thread::current().id();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let task = tokio::spawn(run_blocking(move || {
            started_tx.send(std::thread::current().id()).unwrap();
            release_rx
                .recv_timeout(std::time::Duration::from_secs(3))
                .unwrap();
            Err::<(), _>(crate::error::AppError::msg("fixture failure"))
        }));
        let worker = tokio::time::timeout(std::time::Duration::from_secs(2), started_rx)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(caller, worker);
        // This continuation must be able to release the worker while it waits.
        release_tx.send(()).unwrap();
        assert_eq!(
            task.await.unwrap().unwrap_err().to_string(),
            "fixture failure"
        );
    }
}
