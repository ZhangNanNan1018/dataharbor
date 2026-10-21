mod catalogdiff;
mod commands;
mod crypto;
mod data;
mod dbf;
mod drivers;
mod error;
mod mail;
mod migrate;
mod models;
mod native;
mod parquetio;
mod pdf;
mod pldbg;
mod plan;
mod query;
mod schedule;
mod schema;
mod sqlfmt;
mod sqlutil;
mod state;
mod store;
mod studio;
mod tunnel;

#[cfg(test)]
mod server_tests;

use commands::{
    ai_ask, ai_settings, analyze_schema, app_info, backup_sql, bind_sql, commit_changes, connect,
    create_sample_database, data_dictionary, delete_connection, delete_dashboard, delete_favorite,
    delete_job, delete_model, delete_snippet, diff_structure, disconnect, er_diagram, execute_sql,
    explain_query, export_project, export_sql_csv, export_table_csv, export_table_file, fetch_rows,
    format_sql, forward_sql, generate_rows, import_project, import_table_csv, import_table_file,
    list_accounts, list_columns, list_connections, list_dashboards, list_databases, list_favorites,
    list_jobs, list_models, list_schemas, list_snippets, list_tables, list_views, mask_columns,
    model_html, preview_design, redis_listen, redis_publish, restore_sql, reverse_model,
    run_due_jobs, save_ai_settings, save_connection, save_dashboard, save_favorite, save_job,
    save_model, save_snippet, search_catalog, server_activity, sync_rows, test_connection,
    transfer_rows, explain_plan, foreign_keys, reference_rows, debug_status, debug_launch, debug_step, debug_stop, profile_table, redis_expire, export_pdf_text,
    convert_backup, install_schedule, mail_settings, save_mail_settings,
};
use tauri::Manager;
use state::AppState;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static TRAY_READY: AtomicBool = AtomicBool::new(false);

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if std::env::args().any(|arg| arg == "--run-jobs") {
        match run_jobs_cli() {
            Ok(messages) => {
                for message in messages {
                    println!("{message}");
                }
            }
            Err(err) => {
                eprintln!("计划任务失败: {err}");
                std::process::exit(1);
            }
        }
        return;
    }
    let app = AppState::initialize().unwrap_or_else(|err| {
        eprintln!("DataHarbor 初始化失败: {err}");
        panic!("DataHarbor 初始化失败: {err}");
    });
    eprintln!(
        "dataharbor {} · 凭据：{}",
        env!("CARGO_PKG_VERSION"),
        app.app_info().encryption
    );
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(app)
        .invoke_handler(tauri::generate_handler![
            app_info,
            list_connections,
            save_connection,
            delete_connection,
            test_connection,
            connect,
            disconnect,
            create_sample_database,
            list_databases,
            list_schemas,
            list_tables,
            list_views,
            list_columns,
            execute_sql,
            fetch_rows,
            commit_changes,
            export_table_csv,
            export_sql_csv,
            import_table_csv,
            format_sql,
            bind_sql,
            explain_query,
            list_snippets,
            save_snippet,
            delete_snippet,
            er_diagram,
            export_table_file,
            import_table_file,
            diff_structure,
            transfer_rows,
            sync_rows,
            backup_sql,
            restore_sql,
            generate_rows,
            mask_columns,
            data_dictionary,
            analyze_schema,
            list_accounts,
            server_activity,
            preview_design,
            ai_settings,
            save_ai_settings,
            ai_ask,
            list_jobs,
            save_job,
            delete_job,
            run_due_jobs,
            list_dashboards,
            save_dashboard,
            delete_dashboard,
            list_models,
            save_model,
            delete_model,
            reverse_model,
            forward_sql,
            model_html,
            list_favorites,
            save_favorite,
            delete_favorite,
            search_catalog,
            redis_publish,
            redis_listen,
            export_project,
            import_project,
            explain_plan,
            foreign_keys,
            reference_rows,
            debug_status,
            debug_launch,
            debug_step,
            debug_stop,
            profile_table,
            redis_expire,
            export_pdf_text,
            convert_backup,
            install_schedule,
            mail_settings,
            save_mail_settings,
        ])
        .setup(|app| {
            if let Err(err) = install_tray(app.handle()) {
                eprintln!("托盘没有挂上: {err}");
            } else {
                TRAY_READY.store(true, Ordering::Relaxed);
            }
            let handle = app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(15));
                let state = handle.state::<AppState>();
                if let Err(err) = studio::run_due_jobs(&state) {
                    eprintln!("计划任务失败：{err}");
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if !TRAY_READY.load(Ordering::Relaxed) {
                    return;
                }
                let keep = window
                    .state::<AppState>()
                    .config_dir()
                    .join("background.json");
                let pause = std::fs::read_to_string(keep)
                    .map(|text| text.contains("\"keepAlive\":false") || text.contains("\"keepAlive\": false"))
                    .unwrap_or(false);
                if !pause {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("DataHarbor 启动失败");
}

fn run_jobs_cli() -> crate::error::AppResult<Vec<String>> {
    let app = AppState::initialize()?;
    studio::run_due_jobs(&app)
}

fn install_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::TrayIconBuilder;
    let show = MenuItem::with_id(app, "show", "显示 DataHarbor", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    TrayIconBuilder::new()
        .menu(&menu)
        .tooltip("DataHarbor")
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}
