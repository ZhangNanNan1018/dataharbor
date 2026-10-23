use super::backup::backup_sql_with_operation;
use super::file_io::export_file_with_operation;
use super::storage::{now_secs, store_lock, write_json_unlocked};
use super::types::{
    default_mail_port, BackupRequest, FileExportRequest, Job, MailSave, MailSettings,
};
use crate::error::{AppError, AppResult};
use crate::models::ExecuteSqlRequest;
use crate::query::execute_sql_with_operation;
use crate::operations::OperationContext;
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use uuid::Uuid;

fn read_jobs(state: &AppState) -> AppResult<Vec<Job>> {
    crate::persistence::read_collection(state.config_dir(), "jobs.json")
}

pub fn list_jobs(state: &AppState) -> AppResult<Vec<Job>> {
    let _guard = store_lock(state, "jobs.json")?;
    read_jobs(state)
}

pub(super) fn update_jobs<T>(
    state: &AppState,
    update: impl FnOnce(&mut Vec<Job>) -> AppResult<T>,
) -> AppResult<T> {
    let _guard = store_lock(state, "jobs.json")?;
    let mut jobs = read_jobs(state)?;
    let result = update(&mut jobs)?;
    write_json_unlocked(state, "jobs.json", &crate::persistence::collection(&jobs)?)?;
    Ok(result)
}

pub fn save_job(state: &AppState, mut job: Job) -> AppResult<Job> {
    if job.name.trim().is_empty() {
        return Err(AppError::msg("任务需要名称"));
    }
    if job.id.is_empty() {
        job.id = Uuid::new_v4().to_string();
    }
    if job.interval_secs < 15 {
        job.interval_secs = 15;
    }
    if job.next_run_at == 0 {
        job.next_run_at = now_secs().saturating_add(job.interval_secs);
    }
    job.revision = Uuid::new_v4().to_string();
    update_jobs(state, |items| {
        if let Some(slot) = items.iter_mut().find(|item| item.id == job.id) {
            *slot = job.clone();
        } else {
            items.push(job.clone());
        }
        Ok(job)
    })
}

pub fn delete_job(state: &AppState, id: &str) -> AppResult<()> {
    update_jobs(state, |items| {
        items.retain(|item| item.id != id);
        Ok(())
    })
}

pub fn run_due_jobs(state: &AppState) -> AppResult<Vec<String>> {
    let results = run_due_jobs_with(state, |job, operation| execute_job(state, job, operation))?;
    summarize_runs(results)
}

fn summarize_runs(results: Vec<JobRun>) -> AppResult<Vec<String>> {
    let failed = results.iter().filter(|item| !item.ok).count();
    let messages: Vec<String> = results
        .into_iter()
        .map(|item| format!("{}: {}", item.name, item.message))
        .collect();
    if failed > 0 {
        return Err(AppError::msg(format!(
            "{failed} 个计划任务失败：\n{}",
            messages.join("\n")
        )));
    }
    Ok(messages)
}

#[derive(Debug)]
struct JobRun {
    name: String,
    ok: bool,
    message: String,
}

fn run_due_jobs_with(
    state: &AppState,
    mut execute: impl FnMut(&Job, &OperationContext) -> AppResult<String>,
) -> AppResult<Vec<JobRun>> {
    let run_lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(state.config_dir().join("jobs-run.lock"))
        .map_err(|err| AppError::msg(format!("无法打开任务执行锁：{err}")))?;
    match run_lock.try_lock() {
        Ok(()) => {}
        Err(fs::TryLockError::WouldBlock) => return Ok(Vec::new()),
        Err(fs::TryLockError::Error(err)) => {
            return Err(AppError::msg(format!("无法锁定任务执行：{err}")))
        }
    }
    let uncertain = state.history.uncertain_jobs()?;
    if !uncertain.is_empty() {
        update_jobs(state, |jobs| {
            for record in &uncertain {
                if let Some(job) = jobs.iter_mut().find(|job| Some(&job.id) == record.job_id.as_ref() && Some(&job.revision) == record.revision.as_ref()) {
                    job.enabled = false;
                    job.revision = Uuid::new_v4().to_string();
                    job.last_message = "上次执行异常退出，结果待核对；已暂停自动重试，请核对数据库和输出文件后再启用。".into();
                }
            }
            Ok(())
        })?;
    }
    let now = now_secs();
    let snapshots = list_jobs(state)?;
    let mut messages = Vec::new();
    for snapshot in snapshots
        .into_iter()
        .filter(|job| job.enabled && job.next_run_at <= now)
    {
        if !list_jobs(state)?.iter().any(|current| {
            current.id == snapshot.id && current.enabled && current.revision == snapshot.revision
        }) {
            continue;
        }
        // Database and mail operations run without holding the storage lock.
        let created = state.operations.create(format!("计划任务 · {}", snapshot.name));
        let operation = state.operations.claim(&created.id)?;
        if let Err(error) = operation.record(&state.history, Some((&snapshot.id, &snapshot.revision))) {
            operation.finish::<()>(&Err(crate::error::AppError::msg(error.to_string())));
            return Err(error);
        }
        let result = execute(&snapshot, &operation);
        let ok = result.is_ok();
        let last_message = match &result {
            Ok(text) => text.clone(),
            Err(err) => format!("失败：{err}"),
        };
        let persisted = update_jobs(state, |current| {
            let Some(job) = current.iter_mut().find(|job| {
                job.id == snapshot.id && job.enabled && job.revision == snapshot.revision
            }) else {
                return Ok(());
            };
            job.last_message = last_message.clone();
            job.next_run_at = now_secs().saturating_add(job.interval_secs);
            job.revision = Uuid::new_v4().to_string();
            Ok(())
        });
        if let Err(error) = persisted {
            operation.abandon_record();
            operation.finish::<()>(&Err(AppError::msg(error.to_string())));
            return Err(error);
        }
        operation.finish(&result);
        // Editing a job changes persistence ownership, not the outcome of an operation already run.
        messages.push(JobRun {
            name: snapshot.name,
            ok,
            message: last_message,
        });
    }
    Ok(messages)
}

fn execute_job(state: &AppState, job: &Job, context: &OperationContext) -> AppResult<String> {
    let mut ids = vec![job.connection_id.clone()];
    for id in &job.profile_ids {
        if !ids.contains(id) {
            ids.push(id.clone());
        }
    }
    let mut messages = Vec::new();
    let mut failed = false;
    for (index, id) in ids.iter().enumerate() {
        let mut operation = job.clone();
        operation.connection_id = id.clone();
        if index > 0 && !operation.path.is_empty() {
            operation.path = format!("{}-{}", operation.path, id);
        }
        match run_job_operation(state, &operation, context) {
            Ok(message) => messages.push(format!("{id}: {message}")),
            Err(err) => {
                failed = true;
                messages.push(format!("{id}: {err}"));
            }
        }
    }
    let mut message = messages.join(" | ");
    if !job.notify_email.trim().is_empty() {
        match send_job_mail(state, &job.notify_email, &job.name, &message, &job.path) {
            Ok(()) => message.push_str(" · 已发送邮件"),
            Err(err) => {
                failed = true;
                message.push_str(&format!(" · 邮件未发出: {err}"));
            }
        }
    }
    if failed {
        Err(AppError::msg(message))
    } else {
        Ok(message)
    }
}

fn run_job_operation(state: &AppState, job: &Job, operation: &OperationContext) -> AppResult<String> {
    // Scheduled and desktop jobs share the same authenticated connection path.
    let connection = state.connect(&job.connection_id)?;
    let database = if job.database.is_empty() {
        connection.database
    } else {
        job.database.clone()
    };
    match job.kind.as_str() {
        "backup" => backup_sql_with_operation(
            state,
            &BackupRequest {
                connection_id: job.connection_id.clone(),
                database,
                schema: job.schema.clone(),
                path: if job.path.is_empty() {
                    state
                        .config_dir()
                        .join(format!("{}-{}.sql", job.id, job.connection_id))
                        .display()
                        .to_string()
                } else {
                    job.path.clone()
                },
                mode: "auto".into(),
            },
            operation,
        )
        .map(|item| {
            format!(
                "{}，{} 行。{}",
                if item.kind == "data-only" {
                    "已生成数据脚本"
                } else {
                    "备份操作完成"
                },
                item.row_count,
                item.note
            )
        }),
        "export" => export_file_with_operation(
            state,
            FileExportRequest {
                connection_id: job.connection_id.clone(),
                database,
                schema: job.schema.clone(),
                table: job.table.clone(),
                format: "csv".into(),
                path: job.path.clone(),
            },
            operation,
        )
        .map(|item| format!("导出 {} 行。{}", item.row_count, item.note)),
        "query" => execute_sql_with_operation(
            state,
            ExecuteSqlRequest { session_id: None,
                connection_id: job.connection_id.clone(),
                database,
                sql: job.sql.clone(),
                page: 1,
                page_size: 20,
                only_statement: None,
            },
            operation,
        )
        .and_then(|response| {
            if let Some(failed) = response.statements.iter().find(|item| !item.ok) {
                Err(AppError::msg(
                    failed.error.clone().unwrap_or_else(|| "查询失败".into()),
                ))
            } else {
                Ok(format!("查询完成，{} 条语句", response.statements.len()))
            }
        }),
        _ => Err(AppError::msg("不支持的任务类型")),
    }
}

fn send_job_mail(
    state: &AppState,
    to: &str,
    name: &str,
    body: &str,
    attachment_path: &str,
) -> AppResult<()> {
    let file = read_mail(state)?;
    if file.host.trim().is_empty() || file.from.trim().is_empty() {
        return Err(AppError::msg("先在定时任务里保存 SMTP 主机和发件人"));
    }
    let password = if file.password_enc.is_empty() {
        String::new()
    } else {
        state.decrypt_secret(&file.password_enc)?
    };
    let attachment = if attachment_path.is_empty() {
        Vec::new()
    } else {
        fs::read(attachment_path).unwrap_or_default()
    };
    let name_file = Path::new(attachment_path)
        .file_name()
        .and_then(|item| item.to_str())
        .unwrap_or("result.txt")
        .to_string();
    crate::mail::send_mail(&crate::mail::MailRequest {
        host: file.host,
        port: file.port,
        username: file.username,
        password,
        from: file.from,
        to: to.to_string(),
        subject: format!("DataHarbor · {name}"),
        body: body.to_string(),
        attachment_name: if attachment.is_empty() {
            "note.txt".into()
        } else {
            name_file
        },
        attachment: if attachment.is_empty() {
            body.as_bytes().to_vec()
        } else {
            attachment
        },
        starttls: file.starttls,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct MailFile {
    #[serde(default)]
    host: String,
    #[serde(default = "default_mail_port")]
    port: u16,
    #[serde(default)]
    username: String,
    #[serde(default)]
    password_enc: String,
    #[serde(default)]
    from: String,
    #[serde(default)]
    starttls: bool,
}

fn read_mail(state: &AppState) -> AppResult<MailFile> {
    let path = state.config_dir().join("mail.json");
    if !path.exists() {
        return Ok(MailFile::default());
    }
    let text = fs::read_to_string(path).map_err(|err| AppError::msg(err.to_string()))?;
    serde_json::from_str(&text).map_err(|err| AppError::msg(err.to_string()))
}

pub fn mail_settings(state: &AppState) -> AppResult<MailSettings> {
    let file = read_mail(state)?;
    Ok(MailSettings {
        host: file.host,
        port: file.port,
        username: file.username,
        from: file.from,
        starttls: file.starttls,
        has_password: !file.password_enc.is_empty(),
    })
}

pub fn save_mail_settings(state: &AppState, request: MailSave) -> AppResult<MailSettings> {
    let guard = store_lock(state, "mail.json")?;
    let mut file = read_mail(state)?;
    file.host = request.host.trim().to_string();
    file.port = if request.port == 0 { 587 } else { request.port };
    file.username = request.username.trim().to_string();
    file.from = request.from.trim().to_string();
    file.starttls = request.starttls || file.port != 465;
    if request.clear_password {
        file.password_enc.clear();
    } else if !request.password.trim().is_empty() {
        file.password_enc = state.encrypt_secret(request.password.trim())?;
    }
    write_json_unlocked(state, "mail.json", &file)?;
    drop(guard);
    mail_settings(state)
}

pub fn install_schedule(state: &AppState, interval_minutes: u64) -> AppResult<String> {
    crate::schedule::install_user_schedule(state.config_dir(), interval_minutes.max(1))
}

#[cfg(test)]
mod tests {
    use super::{delete_job, list_jobs, run_due_jobs_with, save_job};
    use crate::state::AppState;
    use crate::studio::project::import_project;
    use crate::studio::types::Job;
    use serde_json::json;
    use std::fs;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    use uuid::Uuid;

    fn job(id: &str) -> Job {
        serde_json::from_value(json!({ "id": id, "name": id, "kind": "query", "connectionId": "fixture", "sql": "SELECT 1", "enabled": true, "nextRunAt": 1 })).unwrap()
    }

    #[test]
    fn running_job_cannot_overwrite_paused_edited_deleted_or_imported_jobs() {
        let dir = std::env::temp_dir().join(format!("dataharbor-job-race-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        {
            let state = AppState::open_in(dir.clone(), true).unwrap();
            for id in ["paused", "deleted", "edited"] {
                save_job(&state, job(id)).unwrap();
            }
            let (started_tx, started_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            std::thread::scope(|scope| {
                let state_ref = &state;
                let worker = scope.spawn(move || {
                    let deadline = Instant::now() + Duration::from_secs(5);
                    loop {
                        let mut entered = false;
                        let result = run_due_jobs_with(state_ref, |snapshot, _| {
                            entered = true;
                            assert_eq!(snapshot.id, "paused");
                            started_tx.send(()).unwrap();
                            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                            Ok("old result".into())
                        })
                        .unwrap();
                        if entered {
                            return result;
                        }
                        assert!(Instant::now() < deadline, "job executor stayed busy");
                        std::thread::yield_now();
                    }
                });
                started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                let mut paused = list_jobs(&state)
                    .unwrap()
                    .into_iter()
                    .find(|job| job.id == "paused")
                    .unwrap();
                paused.enabled = false;
                save_job(&state, paused).unwrap();
                delete_job(&state, "deleted").unwrap();
                let mut edited = job("edited");
                edited.sql = "SELECT 2".into();
                save_job(&state, edited).unwrap();
                let mut new_job = job("new");
                new_job.enabled = false;
                save_job(&state, new_job).unwrap();
                let project = dir.join("project.json");
                fs::write(&project, serde_json::to_vec(&json!({ "version": 1, "profiles": [], "snippets": [], "dashboards": [], "models": [], "favorites": [], "jobs": [job("imported")] })).unwrap()).unwrap();
                import_project(&state, project.to_str().unwrap()).unwrap();
                assert!(run_due_jobs_with(&state, |_, _| panic!(
                    "parallel execution must be skipped"
                ))
                .unwrap()
                .is_empty());
                release_tx.send(()).unwrap();
                let completed = worker.join().unwrap();
                assert_eq!(completed.len(), 1);
                assert_eq!(completed[0].name, "paused");
                assert!(completed[0].ok);
            });
            let jobs = list_jobs(&state).unwrap();
            assert!(!jobs.iter().find(|job| job.id == "paused").unwrap().enabled);
            assert!(!jobs.iter().any(|job| job.id == "deleted"));
            assert_eq!(
                jobs.iter().find(|job| job.id == "edited").unwrap().sql,
                "SELECT 2"
            );
            assert!(!jobs.iter().find(|job| job.id == "new").unwrap().enabled);
            assert!(
                !jobs
                    .iter()
                    .find(|job| job.id == "imported")
                    .unwrap()
                    .enabled
            );
            assert!(jobs.iter().all(|job| job.last_message != "old result"));
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unreadable_job_store_is_preserved_instead_of_replaced() {
        let dir = std::env::temp_dir().join(format!("dataharbor-job-invalid-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        {
            let state = AppState::open_in(dir.clone(), true).unwrap();
            fs::write(dir.join("jobs.json"), "not valid JSON").unwrap();
            assert!(save_job(&state, job("new")).is_err());
            assert_eq!(
                fs::read_to_string(dir.join("jobs.json")).unwrap(),
                "not valid JSON"
            );
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn paused_during_execution_still_reports_failure_to_cli_summary() {
        let dir =
            std::env::temp_dir().join(format!("dataharbor-paused-failure-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        {
            let state = AppState::open_in(dir.clone(), true).unwrap();
            save_job(&state, job("paused-failure")).unwrap();
            let results = run_due_jobs_with(&state, |snapshot, _| {
                let mut paused = snapshot.clone();
                paused.enabled = false;
                paused.last_message = "用户暂停".into();
                save_job(&state, paused).unwrap();
                Err(crate::error::AppError::msg("操作实际失败"))
            })
            .unwrap();
            let error = super::summarize_runs(results).unwrap_err().to_string();
            assert!(error.contains("1 个计划任务失败"));
            assert!(error.contains("操作实际失败"));
            let paused = list_jobs(&state).unwrap().remove(0);
            assert!(!paused.enabled);
            assert_eq!(paused.last_message, "用户暂停");
            assert_eq!(paused.next_run_at, 1);
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cold_start_jobs_connect_and_report_real_success_and_failure() {
        use crate::models::{Engine, SaveConnectionRequest};
        let dir = std::env::temp_dir().join(format!("dataharbor-cold-jobs-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let db = dir.join("fixture.sqlite");
        let id;
        {
            let state = AppState::open_in(dir.clone(), true).unwrap();
            id = state
                .save_connection(SaveConnectionRequest {
                    name: "fixture".into(),
                    engine: Engine::Sqlite,
                    file_path: db.display().to_string(),
                    ..Default::default()
                })
                .unwrap()
                .connection
                .id;
            let mut query = job("query");
            query.connection_id = id.clone();
            query.database = "main".into();
            query.sql = "CREATE TABLE items(id INTEGER PRIMARY KEY, value TEXT); INSERT INTO items VALUES(1, 'cold start');".into();
            save_job(&state, query).unwrap();
            let mut export = job("export");
            export.connection_id = id.clone();
            export.kind = "export".into();
            export.database = "main".into();
            export.schema = "main".into();
            export.table = "items".into();
            export.path = dir.join("export.csv").display().to_string();
            save_job(&state, export).unwrap();
        }
        {
            let state = AppState::open_in(dir.clone(), true).unwrap();
            assert!(state.list_connections().iter().all(|item| !item.connected));
            assert_eq!(super::run_due_jobs(&state).unwrap().len(), 2);
            assert!(fs::read_to_string(dir.join("export.csv"))
                .unwrap()
                .contains("cold start"));
            let check = rusqlite::Connection::open(&db).unwrap();
            assert_eq!(
                check
                    .query_row("SELECT COUNT(*) FROM items", [], |row| row.get::<_, i64>(0))
                    .unwrap(),
                1
            );
            let mut broken = job("broken");
            broken.connection_id = id;
            broken.sql = "INSERT INTO missing_table VALUES(1)".into();
            save_job(&state, broken).unwrap();
            let error = super::run_due_jobs(&state).unwrap_err().to_string();
            assert!(error.contains("1 个计划任务失败"));
            let saved = list_jobs(&state)
                .unwrap()
                .into_iter()
                .find(|item| item.id == "broken")
                .unwrap();
            assert!(saved.last_message.starts_with("失败："));
            assert!(saved.next_run_at > 1);
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn task_run_lock_excludes_another_process_and_releases_on_close() {
        const ENV: &str = "DATAHARBOR_TEST_TASK_LOCK_DIR";
        if let Some(dir) = std::env::var_os(ENV) {
            let state = AppState::open_in(dir.into(), true).unwrap();
            assert!(
                run_due_jobs_with(&state, |_, _| panic!("another process owns the lock"))
                    .unwrap()
                    .is_empty()
            );
            return;
        }
        let dir = std::env::temp_dir().join(format!("dataharbor-job-process-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        {
            let state = AppState::open_in(dir.clone(), true).unwrap();
            save_job(&state, job("due")).unwrap();
            let lock = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(dir.join("jobs-run.lock"))
                .unwrap();
            lock.lock().unwrap();
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "studio::automation::tests::task_run_lock_excludes_another_process_and_releases_on_close", "--nocapture"])
                .env(ENV, &dir).output().unwrap();
            assert!(
                child.status.success(),
                "{}",
                String::from_utf8_lossy(&child.stdout)
            );
            drop(lock);
            let result = run_due_jobs_with(&state, |_, _| Ok("once".into())).unwrap();
            assert_eq!(result.len(), 1);
            assert!(run_due_jobs_with(&state, |_, _| panic!("already advanced"))
                .unwrap()
                .is_empty());
        }
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn crashed_schedule_is_paused_until_user_reenables_a_new_revision() {
        let dir = std::env::temp_dir().join(format!("dataharbor-job-recovery-{}",Uuid::new_v4()));
        let state = AppState::open_in(dir.clone(),true).unwrap();
        let saved = save_job(&state,job("recover")).unwrap();
        let id = Uuid::new_v4().to_string();
        let lease = state.history.start(&id,Some((&saved.id,&saved.revision))).unwrap();
        lease.progress("写入事务",500,1).unwrap();
        drop(lease);
        assert!(run_due_jobs_with(&state, |_, _| panic!("must not replay uncertain writes")).unwrap().is_empty());
        let mut paused = list_jobs(&state).unwrap().remove(0);
        assert!(!paused.enabled);
        assert!(paused.last_message.contains("结果待核对"));
        let record = state.history.list().unwrap().remove(0);
        assert_eq!((record.status.as_str(),record.committed),("unknown",500));
        paused.enabled = true;
        paused.next_run_at = 1;
        save_job(&state,paused).unwrap();
        assert_eq!(run_due_jobs_with(&state, |_, _| Ok("explicit new run".into())).unwrap().len(),1);
        drop(state);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn schedule_write_failure_keeps_the_completed_database_work_uncertain() {
        let dir = std::env::temp_dir().join(format!("dataharbor-job-store-failure-{}",Uuid::new_v4()));
        let state = AppState::open_in(dir.clone(),true).unwrap();
        save_job(&state,job("failure")).unwrap();
        let before = fs::read(dir.join("jobs.json")).unwrap();
        let mut calls = 0;
        assert!(run_due_jobs_with(&state, |_, operation| {
            calls += 1;
            operation.committed(3);
            fs::write(dir.join("jobs.json"), "broken store").unwrap();
            Ok("database already changed".into())
        }).is_err());
        assert_eq!(calls,1);
        fs::write(dir.join("jobs.json"), before).unwrap();
        assert!(run_due_jobs_with(&state, |_, _| panic!("must not replay")).unwrap().is_empty());
        assert!(!list_jobs(&state).unwrap()[0].enabled);
        drop(state);
        fs::remove_dir_all(dir).unwrap();
    }

}
