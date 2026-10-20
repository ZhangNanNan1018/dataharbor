use super::storage::{engine_of, profile_of};
use super::table_data::{ensure_complete, load_all_with_ceiling, CEILING};
use super::types::{BackupRequest, FileExportResult, RestoreRequest};
use crate::error::{AppError, AppResult};
use crate::models::{Engine, ExecuteSqlRequest};
use crate::query::execute_sql;
use crate::sqlutil::{qualified_name, quote_ident, sql_literal, value_sql};
use crate::state::AppState;
use serde_json::Value as JsonValue;
use std::fs;
use std::path::Path;

pub fn backup_sql(state: &AppState, request: &BackupRequest) -> AppResult<FileExportResult> {
    backup_with_ceiling(state, request, CEILING)
}

pub(super) fn backup_with_ceiling(
    state: &AppState,
    request: &BackupRequest,
    ceiling: u32,
) -> AppResult<FileExportResult> {
    let engine = engine_of(state, &request.connection_id)?;
    let mode = request.mode.to_ascii_lowercase();
    if mode != "builtin" {
        if let Some(result) = try_native_backup(state, request, engine)? {
            return Ok(result);
        }
        if mode == "native" {
            return Err(AppError::msg("已指定只用本机工具，但工具不存在或执行失败。可改成自动模式以回退到内置 INSERT 脚本。"));
        }
    }
    if engine == Engine::Mongodb || engine == Engine::Redis {
        return Err(AppError::msg(
            "没有找到 mongodump。文档库请用 JSON 导出，或安装 mongodump 后把备份模式设为本机工具。",
        ));
    }
    if !engine.is_sql() {
        return Err(AppError::msg(
            "文档库请用 JSON 导出。逻辑备份目前覆盖 SQL 引擎。",
        ));
    }
    let tables = state.with_session(&request.connection_id, |session| {
        session.list_tables(&request.database, &request.schema)
    })?;
    let (kind, note) = if engine == Engine::Sqlite {
        ("sqlite-script", "SQLite 表结构和数据脚本；不含独立索引、视图和触发器。读取期间需停止写入，本脚本不提供一致性快照。")
    } else {
        ("data-only", "仅含 INSERT 数据，目标端必须预先具备兼容表结构。不包含建表、索引、约束、权限等对象，不是完整数据库备份。读取期间需停止写入。")
    };
    let mut script = format!(
        "-- DataHarbor {kind}\n-- engine: {}\n-- {note}\n\n",
        engine.label()
    );
    let mut count = 0u64;
    for table in &tables {
        let (columns, rows, total, truncated) = load_all_with_ceiling(
            state,
            &request.connection_id,
            &request.database,
            &request.schema,
            table,
            ceiling,
        )?;
        ensure_complete(
            total,
            rows.len(),
            truncated,
            &format!("内置备份（表 {table}）"),
        )?;
        let name = qualified_name(engine, &request.database, &request.schema, table)?;
        script.push_str(&format!("-- {name}\n"));
        if engine == Engine::Sqlite {
            let outcome = state.with_session(&request.connection_id, |session| {
                session.execute(
                    &request.database,
                    &format!(
                        "SELECT sql FROM sqlite_master WHERE name = {}",
                        sql_literal(Engine::Sqlite, &JsonValue::String(table.clone()))?
                    ),
                    Some(1),
                )
            })?;
            let definition = outcome
                .rows
                .first()
                .and_then(|row| row.first())
                .and_then(|value| value.as_str())
                .ok_or_else(|| AppError::msg(format!("无法读取 {table} 的建表语句，已取消备份")))?;
            script.push_str(definition);
            script.push_str(";\n");
        }
        for row in &rows {
            let values = row
                .iter()
                .zip(&columns)
                .map(|(value, column)| value_sql(engine, &column.data_type, value))
                .collect::<AppResult<Vec<_>>>()?
                .join(", ");
            let cols = columns
                .iter()
                .map(|column| quote_ident(engine, &column.name))
                .collect::<AppResult<Vec<_>>>()?
                .join(", ");
            script.push_str(&format!("INSERT INTO {name} ({cols}) VALUES ({values});\n"));
            count += 1;
        }
        script.push('\n');
    }
    fs::write(&request.path, script).map_err(|err| AppError::msg(err.to_string()))?;
    Ok(FileExportResult {
        path: request.path.clone(),
        row_count: count,
        truncated: false,
        kind: kind.into(),
        note: note.into(),
    })
}

pub fn restore_sql(state: &AppState, request: &RestoreRequest) -> AppResult<String> {
    let engine = engine_of(state, &request.connection_id)?;
    if engine == Engine::Mssql {
        let head = fs::read(&request.path).unwrap_or_default();
        if !head.starts_with(b"--")
            && !head.starts_with(b"CREATE")
            && !head.starts_with(b"INSERT")
            && !head.starts_with(b"/*")
            && !head.starts_with(&[0xEF, 0xBB, 0xBF])
        {
            let sql = crate::native::mssql_restore_sql(&request.database, &request.path);
            let response = execute_sql(
                state,
                ExecuteSqlRequest {
                    connection_id: request.connection_id.clone(),
                    database: request.database.clone(),
                    sql,
                    page: 1,
                    page_size: 5,
                    only_statement: None,
                },
            )?;
            if let Some(failed) = response.statements.iter().find(|item| !item.ok) {
                return Err(AppError::msg(
                    failed
                        .error
                        .clone()
                        .unwrap_or_else(|| "RESTORE 失败".into()),
                ));
            }
            return Ok(
                "已提交 SQL Server RESTORE。备份文件必须位于数据库服务器能访问的路径。".into(),
            );
        }
    }
    if engine == Engine::Mongodb {
        if crate::native::tool_path("mongorestore").is_some() {
            let profile = profile_of(state, &request.connection_id)?;
            let password = state
                .decrypt_secret(&profile.password_enc)
                .unwrap_or_default();
            let mut command = std::process::Command::new("mongorestore");
            command.args([
                "--archive",
                &request.path,
                "--host",
                &profile.host,
                "--port",
                &profile.port.to_string(),
            ]);
            if !profile.username.is_empty() {
                command.args([
                    "--username",
                    &profile.username,
                    "--password",
                    &password,
                    "--authenticationDatabase",
                    "admin",
                ]);
            }
            let output = command
                .output()
                .map_err(|err| AppError::msg(err.to_string()))?;
            if !output.status.success() {
                return Err(AppError::msg(format!(
                    "mongorestore 失败：{}",
                    String::from_utf8_lossy(&output.stderr)
                )));
            }
            return Ok("已用 mongorestore 还原归档".into());
        }
    }
    let meta = fs::metadata(&request.path).map_err(|err| AppError::msg(err.to_string()))?;
    if meta.len() > 20 * 1024 * 1024 {
        return Err(AppError::msg("SQL 文件超过 20MB"));
    }
    let sql = fs::read_to_string(&request.path).map_err(|err| AppError::msg(err.to_string()))?;
    let response = execute_sql(
        state,
        ExecuteSqlRequest {
            connection_id: request.connection_id.clone(),
            database: request.database.clone(),
            sql,
            page: 1,
            page_size: 20,
            only_statement: None,
        },
    )?;
    let failed = response.statements.iter().filter(|item| !item.ok).count();
    let ok = response.statements.len() - failed;
    if failed > 0 {
        let message = response
            .statements
            .iter()
            .find(|item| !item.ok)
            .and_then(|item| item.error.clone())
            .unwrap_or_default();
        return Err(AppError::msg(format!(
            "已执行 {ok} 条，失败 {failed} 条：{message}"
        )));
    }
    Ok(format!("已执行 {ok} 条语句"))
}

pub fn convert_backup(source: &str, destination: &str) -> AppResult<String> {
    crate::native::convert_dump_to_sql(Path::new(source), Path::new(destination))
}

fn try_native_backup(
    state: &AppState,
    request: &BackupRequest,
    engine: Engine,
) -> AppResult<Option<FileExportResult>> {
    let Some(profile) = state
        .profiles_snapshot()
        .into_iter()
        .find(|item| item.id == request.connection_id)
    else {
        return Ok(None);
    };
    let password = if profile.password_enc.is_empty() {
        String::new()
    } else {
        state
            .decrypt_secret(&profile.password_enc)
            .unwrap_or_default()
    };
    let port = if profile.port == 0 {
        engine.default_port()
    } else {
        profile.port
    };
    match engine {
        Engine::Mysql | Engine::Mariadb if crate::native::tool_path("mysqldump").is_some() => {
            let secret = state.config_dir().join(format!("dump-{}.cnf", profile.id));
            crate::native::write_secret(
                &secret,
                &crate::native::mysql_defaults(&profile.host, port, &profile.username, &password),
            )?;
            let plan = crate::native::mysql_plan(
                &profile.host,
                port,
                &profile.username,
                &request.database,
                &request.path,
                &secret.display().to_string(),
            );
            match crate::native::run_plan(&plan) {
                Ok(_note) => Ok(Some(FileExportResult {
                    path: request.path.clone(),
                    row_count: 0,
                    truncated: false,
                    kind: "native-backup".into(),
                    note: "已调用数据库原生备份机制。请在隔离数据库中验证可还原性。".into(),
                })),
                Err(err) => {
                    let _ = fs::remove_file(&secret);
                    if request.mode.eq_ignore_ascii_case("native") {
                        Err(err)
                    } else {
                        Ok(None)
                    }
                }
            }
        }
        Engine::Postgres if crate::native::tool_path("pg_dump").is_some() => {
            let database = if request.database.is_empty() {
                profile.database.clone()
            } else {
                request.database.clone()
            };
            let plan = crate::native::pg_plan(
                &profile.host,
                port,
                &profile.username,
                &database,
                &request.path,
                &password,
            );
            match crate::native::run_plan(&plan) {
                Ok(_) => Ok(Some(FileExportResult {
                    path: request.path.clone(),
                    row_count: 0,
                    truncated: false,
                    kind: "native-backup".into(),
                    note: "已调用数据库原生备份机制。请在隔离数据库中验证可还原性。".into(),
                })),
                Err(err) => {
                    if request.mode.eq_ignore_ascii_case("native") {
                        Err(err)
                    } else {
                        Ok(None)
                    }
                }
            }
        }
        Engine::Mongodb if crate::native::tool_path("mongodump").is_some() => {
            let plan = crate::native::mongo_plan(
                &profile.host,
                port,
                &profile.username,
                &password,
                &request.database,
                &request.path,
            );
            match crate::native::run_plan(&plan) {
                Ok(_) => Ok(Some(FileExportResult {
                    path: request.path.clone(),
                    row_count: 0,
                    truncated: false,
                    kind: "native-backup".into(),
                    note: "已调用数据库原生备份机制。请在隔离数据库中验证可还原性。".into(),
                })),
                Err(err) => {
                    if request.mode.eq_ignore_ascii_case("native") {
                        Err(err)
                    } else {
                        Ok(None)
                    }
                }
            }
        }
        Engine::Mssql => {
            let sql = crate::native::mssql_backup_sql(&request.database, &request.path);
            let response = execute_sql(
                state,
                ExecuteSqlRequest {
                    connection_id: request.connection_id.clone(),
                    database: request.database.clone(),
                    sql,
                    page: 1,
                    page_size: 5,
                    only_statement: None,
                },
            )?;
            if let Some(failed) = response.statements.iter().find(|item| !item.ok) {
                let message = failed.error.clone().unwrap_or_else(|| "BACKUP 失败".into());
                if request.mode.eq_ignore_ascii_case("native") {
                    return Err(AppError::msg(message));
                }
                return Ok(None);
            }
            Ok(Some(FileExportResult {
                path: request.path.clone(),
                row_count: 0,
                truncated: false,
                kind: "native-backup".into(),
                note: "SQL Server 备份文件位于数据库服务器路径，请验证可还原性。".into(),
            }))
        }
        _ => Ok(None),
    }
}
