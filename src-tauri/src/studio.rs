//! Studio commands, grouped by domain. Public names remain stable for the Tauri API.

mod ai;
mod automation;
mod backup;
mod catalog;
mod engine_tools;
mod file_io;
mod inspection;
mod modeling;
mod project;
mod query_tools;
mod storage;
mod sync;
mod table_data;
#[cfg(test)]
mod tests;
mod types;

pub use ai::{ai_ask, ai_settings, save_ai_settings};
pub use automation::{
    delete_job, install_schedule, list_jobs, mail_settings, run_due_jobs, save_job,
    save_mail_settings,
};
pub use backup::{backup_sql, convert_backup, restore_sql};
pub use catalog::{data_dictionary, er_diagram, foreign_keys, reference_rows, search_catalog};
pub use engine_tools::{debug_status, redis_expire, redis_listen, redis_publish};
pub use file_io::{export_file, export_pdf_text, import_file};
pub use inspection::{analyze_schema, profile_table};
pub use modeling::{
    delete_dashboard, delete_model, forward_sql, list_dashboards, list_models, model_html,
    reverse_model, save_dashboard, save_model,
};
pub use project::{
    delete_favorite, delete_snippet, export_project, import_project, list_favorites, list_snippets,
    save_favorite, save_snippet,
};
pub use query_tools::{
    bind_sql, explain_plan, explain_query, format_sql, list_accounts, preview_design,
    server_activity,
};
pub use sync::{diff_structure, sync_rows, transfer_rows};
pub use table_data::{generate_rows, mask_columns};
pub use types::{
    AiAskRequest, AiSettingsPublic, AiSettingsRequest, BackupRequest, BindRequest, Dashboard,
    DataModel, DebugReport, DesignRequest, DiffResult, ErGraph, Favorite, FileExportRequest,
    FileExportResult, FileImportRequest, ForeignKeyRef, GenerateRequest, GridResult, Job, MailSave,
    MailSettings, MaskRequest, ObjectRef, PairRequest, RestoreRequest, Snippet, SqlText,
    SyncResult,
};
