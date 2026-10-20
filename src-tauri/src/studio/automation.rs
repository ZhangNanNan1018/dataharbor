use super::backup::backup_sql;
use super::file_io::export_file;
use super::storage::{now_secs, write_json, write_vec};
use super::types::{
    default_mail_port, BackupRequest, FileExportRequest, Job, MailSave, MailSettings,
};
use crate::error::{AppError, AppResult};
use crate::models::ExecuteSqlRequest;
use crate::query::execute_sql;
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::sync::{Mutex, TryLockError};
use uuid::Uuid;

static JOBS_STORE_LOCK: Mutex<()> = Mutex::new(());
static JOBS_RUN_LOCK: Mutex<()> = Mutex::new(());

fn read_jobs(state: &AppState) -> AppResult<Vec<Job>> {
    let path = state.config_dir().join("jobs.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(path).map_err(|err| AppError::msg(err.to_string()))?;
    serde_json::from_str(&text)
        .map_err(|err| AppError::msg(format!("任务文件无法读取，已停止更新以保留原文件：{err}")))
}

pub fn list_jobs(state: &AppState) -> AppResult<Vec<Job>> {
    let _guard = JOBS_STORE_LOCK
        .lock()
        .map_err(|_| AppError::msg("任务存储锁异常"))?;
    read_jobs(state)
}

pub(super) fn update_jobs<T>(
    state: &AppState,
    update: impl FnOnce(&mut Vec<Job>) -> AppResult<T>,
) -> AppResult<T> {
    let _guard = JOBS_STORE_LOCK
        .lock()
        .map_err(|_| AppError::msg("任务存储锁异常"))?;
    let mut jobs = read_jobs(state)?;
    let result = update(&mut jobs)?;
    write_vec(state, "jobs.json", &jobs)?;
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
    run_due_jobs_with(state, |job| execute_job(state, job))
}

fn run_due_jobs_with(
    state: &AppState,
    mut execute: impl FnMut(&Job) -> String,
) -> AppResult<Vec<String>> {
    let _run_guard = match JOBS_RUN_LOCK.try_lock() {
        Ok(guard) => guard,
        Err(TryLockError::WouldBlock) => return Ok(Vec::new()),
        Err(TryLockError::Poisoned(_)) => return Err(AppError::msg("任务执行锁异常")),
    };
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
        let last_message = execute(&snapshot);
        let merged = update_jobs(state, |current| {
            let Some(job) = current.iter_mut().find(|job| {
                job.id == snapshot.id && job.enabled && job.revision == snapshot.revision
            }) else {
                return Ok(false);
            };
            job.last_message = last_message.clone();
            job.next_run_at = now_secs().saturating_add(job.interval_secs);
            job.revision = Uuid::new_v4().to_string();
            Ok(true)
        })?;
        if merged {
            messages.push(format!("{}: {last_message}", snapshot.name));
        }
    }
    Ok(messages)
}

fn execute_job(state: &AppState, snapshot: &Job) -> String {
    let mut job = snapshot.clone();
    let result = match job.kind.as_str() {
        "backup" => backup_sql(
            state,
            &BackupRequest {
                connection_id: job.connection_id.clone(),
                database: job.database.clone(),
                schema: job.schema.clone(),
                path: if job.path.is_empty() {
                    state
                        .config_dir()
                        .join(format!("{}.sql", job.id))
                        .display()
                        .to_string()
                } else {
                    job.path.clone()
                },
                mode: "auto".into(),
            },
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
        "export" => export_file(
            state,
            FileExportRequest {
                connection_id: job.connection_id.clone(),
                database: job.database.clone(),
                schema: job.schema.clone(),
                table: job.table.clone(),
                format: "csv".into(),
                path: job.path.clone(),
            },
        )
        .map(|item| format!("导出 {} 行。{}", item.row_count, item.note)),
        _ => execute_sql(
            state,
            ExecuteSqlRequest {
                connection_id: job.connection_id.clone(),
                database: job.database.clone(),
                sql: job.sql.clone(),
                page: 1,
                page_size: 20,
                only_statement: None,
            },
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
    };
    job.last_message = match &result {
        Ok(text) => text.clone(),
        Err(err) => err.to_string(),
    };
    if !job.profile_ids.is_empty() {
        let mut extra = Vec::new();
        for id in job.profile_ids.clone() {
            if id == job.connection_id {
                continue;
            }
            let mut clone = job.clone();
            clone.connection_id = id.clone();
            clone.profile_ids.clear();
            clone.notify_email.clear();
            let piece = run_saved_job(state, &clone);
            extra.push(format!("{id}: {piece}"));
        }
        if !extra.is_empty() {
            job.last_message = format!("{} | {}", job.last_message, extra.join(" | "));
        }
    }
    if !job.notify_email.trim().is_empty() {
        match send_job_mail(
            state,
            &job.notify_email,
            &job.name,
            &job.last_message,
            &job.path,
        ) {
            Ok(()) => job.last_message.push_str(" · 已发送邮件"),
            Err(err) => job.last_message.push_str(&format!(" · 邮件未发出: {err}")),
        }
    }
    job.last_message
}

fn run_saved_job(state: &AppState, job: &Job) -> String {
    let result = match job.kind.as_str() {
        "backup" => backup_sql(
            state,
            &BackupRequest {
                connection_id: job.connection_id.clone(),
                database: job.database.clone(),
                schema: job.schema.clone(),
                path: if job.path.is_empty() {
                    state
                        .config_dir()
                        .join(format!("{}-{}.sql", job.id, job.connection_id))
                        .display()
                        .to_string()
                } else {
                    format!("{}-{}", job.path, job.connection_id)
                },
                mode: "auto".into(),
            },
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
        "export" => export_file(
            state,
            FileExportRequest {
                connection_id: job.connection_id.clone(),
                database: job.database.clone(),
                schema: job.schema.clone(),
                table: job.table.clone(),
                format: "csv".into(),
                path: format!("{}-{}.csv", job.path, job.connection_id),
            },
        )
        .map(|item| format!("导出 {} 行。{}", item.row_count, item.note)),
        _ => execute_sql(
            state,
            ExecuteSqlRequest {
                connection_id: job.connection_id.clone(),
                database: job.database.clone(),
                sql: job.sql.clone(),
                page: 1,
                page_size: 20,
                only_statement: None,
            },
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
    };
    match result {
        Ok(text) => text,
        Err(err) => err.to_string(),
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
    write_json(state, "mail.json", &file)?;
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
                        let result = run_due_jobs_with(state_ref, |snapshot| {
                            entered = true;
                            assert_eq!(snapshot.id, "paused");
                            started_tx.send(()).unwrap();
                            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                            "old result".into()
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
                assert!(run_due_jobs_with(&state, |_| panic!(
                    "parallel execution must be skipped"
                ))
                .unwrap()
                .is_empty());
                release_tx.send(()).unwrap();
                assert!(worker.join().unwrap().is_empty());
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
}
