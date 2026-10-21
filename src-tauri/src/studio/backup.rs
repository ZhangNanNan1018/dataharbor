use super::storage::{engine_of, profile_of};
use super::table_data::{ensure_complete, load_all_with_ceiling, CEILING};
use super::types::{BackupRequest, FileExportResult, RestoreRequest};
use crate::error::{AppError, AppResult};
use crate::models::{Engine, ExecuteSqlRequest, StoredProfile};
use crate::query::execute_sql;
use crate::sqlutil::{qualified_name, quote_ident, sql_literal, value_sql};
use crate::state::AppState;
use serde_json::Value as JsonValue;
use std::fs;
use std::io::{Read, Write};
use std::path::Path;

const MAX_STATEMENT_BYTES: usize = 20 * 1024 * 1024;

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
    let header = format!(
        "-- DataHarbor {kind}\n-- engine: {}\n-- {note}\n\n",
        engine.label()
    );
    let temporary =
        Path::new(&request.path).with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let _cleanup = Cleanup(temporary.clone());
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|err| AppError::msg(err.to_string()))?;
    let mut writer = std::io::BufWriter::new(file);
    writer
        .write_all(header.as_bytes())
        .map_err(|err| AppError::msg(err.to_string()))?;
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
        writeln!(writer, "-- {name}").map_err(|err| AppError::msg(err.to_string()))?;
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
            write_statement(&mut writer, definition)?;
        }
        let cols = columns
            .iter()
            .map(|column| quote_ident(engine, &column.name))
            .collect::<AppResult<Vec<_>>>()?
            .join(", ");
        for row in &rows {
            let values = row
                .iter()
                .zip(&columns)
                .map(|(value, column)| value_sql(engine, &column.data_type, value))
                .collect::<AppResult<Vec<_>>>()?
                .join(", ");
            write_statement(
                &mut writer,
                &format!("INSERT INTO {name} ({cols}) VALUES ({values})"),
            )?;
            count += 1;
        }
        writeln!(writer).map_err(|err| AppError::msg(err.to_string()))?;
    }
    writer
        .flush()
        .map_err(|err| AppError::msg(err.to_string()))?;
    drop(writer);
    fs::rename(temporary, &request.path).map_err(|err| AppError::msg(err.to_string()))?;
    Ok(FileExportResult {
        path: request.path.clone(),
        row_count: count,
        truncated: false,
        kind: kind.into(),
        note: note.into(),
    })
}

fn write_statement(writer: &mut impl Write, statement: &str) -> AppResult<()> {
    if statement.len() + 2 > MAX_STATEMENT_BYTES {
        return Err(AppError::msg(
            "单条备份 SQL 超过内置恢复支持的 20MB，未替换目标文件；请使用原生备份工具",
        ));
    }
    writeln!(writer, "{};", statement.trim_end_matches(';'))
        .map_err(|err| AppError::msg(err.to_string()))
}

pub fn restore_sql(state: &AppState, request: &RestoreRequest) -> AppResult<String> {
    let engine = engine_of(state, &request.connection_id)?;
    let mut header = [0; 256];
    let length = fs::File::open(&request.path).and_then(|mut file| file.read(&mut header));
    if engine == Engine::Mssql {
        let head = &header[..length.as_ref().copied().unwrap_or(0)];
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
    let length = length.map_err(|err| AppError::msg(err.to_string()))?;
    let internal = header[..length].starts_with(b"-- DataHarbor ");
    if !internal
        && matches!(
            engine,
            Engine::Postgres | Engine::Mysql | Engine::Mariadb | Engine::Mongodb
        )
    {
        return restore_native(
            state,
            request,
            engine,
            header[..length].starts_with(b"PGDMP"),
        );
    }
    // Preflight all lexical boundaries before a potentially destructive first statement.
    // The file is streamed twice; memory is bounded by one statement, not total file size.
    visit_sql_file(Path::new(&request.path), |_| Ok(()))?;
    state.with_session(&request.connection_id, |session| {
        let mut applied = 0usize;
        visit_sql_file(Path::new(&request.path), |sql| {
            session
                .execute(&request.database, sql, Some(0))
                .map_err(|err| {
                    AppError::msg(format!(
                        "还原在第 {} 条语句失败；前 {applied} 条可能已生效：{err}",
                        applied + 1
                    ))
                })?;
            applied += 1;
            Ok(())
        })?;
        Ok(format!("已执行 {applied} 条语句"))
    })
}

fn visit_sql_file(path: &Path, mut visit: impl FnMut(&str) -> AppResult<()>) -> AppResult<()> {
    let mut file = fs::File::open(path).map_err(|err| AppError::msg(err.to_string()))?;
    let mut pending = Vec::new();
    let mut chunk = [0; 64 * 1024];
    loop {
        let read = file
            .read(&mut chunk)
            .map_err(|err| AppError::msg(err.to_string()))?;
        pending.extend_from_slice(&chunk[..read]);
        let valid_length = match std::str::from_utf8(&pending) {
            Ok(_) => pending.len(),
            Err(err) if err.error_len().is_none() && read != 0 => err.valid_up_to(),
            Err(_) => return Err(AppError::msg("SQL 文件不是有效 UTF-8")),
        };
        let text = std::str::from_utf8(&pending[..valid_length])
            .map_err(|err| AppError::msg(err.to_string()))?;
        let prefix = crate::sqlutil::terminated_sql_prefix(text).unwrap_or(0);
        if prefix > 0 {
            for sql in crate::sqlutil::split_sql(&text[..prefix]) {
                if crate::sqlutil::strip_leading_comments(&sql).len() > MAX_STATEMENT_BYTES {
                    return Err(AppError::msg(
                        "单条 SQL 超过 20MB，请使用数据库原生恢复工具",
                    ));
                }
                visit(&sql)?;
            }
            pending.drain(..prefix);
        }
        if pending.len() > MAX_STATEMENT_BYTES + chunk.len() {
            return Err(AppError::msg(
                "单条 SQL 超过 20MB，请使用数据库原生恢复工具",
            ));
        }
        if read == 0 {
            let tail =
                std::str::from_utf8(&pending).map_err(|err| AppError::msg(err.to_string()))?;
            if crate::sqlutil::strip_leading_comments(tail).len() > MAX_STATEMENT_BYTES {
                return Err(AppError::msg(
                    "单条 SQL 超过 20MB，请使用数据库原生恢复工具",
                ));
            }
            if !crate::sqlutil::sql_is_complete(tail) {
                return Err(AppError::msg("SQL 文件包含未闭合的引号或注释，已停止还原"));
            }
            for sql in crate::sqlutil::split_sql(tail) {
                visit(&sql)?;
            }
            return Ok(());
        }
    }
}

fn validate_native_profile(profile: &StoredProfile) -> AppResult<()> {
    if profile.ssh_enabled || profile.http_tunnel {
        return Err(AppError::msg("当前原生工具无法沿用已验证的隧道，已拒绝直连及自动降级；请明确选择内置脚本，或在可信环境中使用原生客户端"));
    }
    if profile.use_ssl {
        return Err(AppError::msg("当前原生工具无法保证使用与应用一致的 TLS 证书信任配置，已拒绝执行及自动降级；请明确选择内置脚本，或配置原生客户端后恢复"));
    }
    if !matches!(profile.auth_method.as_str(), "" | "password") {
        return Err(AppError::msg(
            "原生工具尚不能等价传递此认证方式，已拒绝执行",
        ));
    }
    Ok(())
}

fn secret_path(state: &AppState, suffix: &str) -> String {
    state
        .config_dir()
        .join(format!("native-{}.{}", uuid::Uuid::new_v4(), suffix))
        .display()
        .to_string()
}

fn restore_native(
    state: &AppState,
    request: &RestoreRequest,
    engine: Engine,
    custom: bool,
) -> AppResult<String> {
    let profile = profile_of(state, &request.connection_id)?;
    validate_native_profile(&profile)?;
    let password = state.decrypt_secret(&profile.password_enc)?;
    let port = if profile.port == 0 {
        engine.default_port()
    } else {
        profile.port
    };
    let database = if request.database.is_empty() {
        &profile.database
    } else {
        &request.database
    };
    let plan = match engine {
        Engine::Postgres => {
            if custom {
                crate::native::pg_restore_plan(
                    &profile.host,
                    port,
                    &profile.username,
                    database,
                    &request.path,
                    &password,
                    true,
                )
            } else {
                let script = secret_path(state, "sql");
                crate::native::prepare_pg_script(Path::new(&request.path), Path::new(&script))?;
                let mut plan = crate::native::pg_restore_plan(
                    &profile.host,
                    port,
                    &profile.username,
                    database,
                    &script,
                    &password,
                    false,
                );
                plan.secret_file = Some(script);
                plan
            }
        }
        Engine::Mysql | Engine::Mariadb => {
            let secret = secret_path(state, "cnf");
            crate::native::write_secret(
                Path::new(&secret),
                &crate::native::mysql_defaults(&profile.host, port, &profile.username, &password),
            )?;
            crate::native::mysql_restore_plan(&secret, database, &request.path)
        }
        Engine::Mongodb => {
            let secret = secret_path(state, "yaml");
            let body = format!(
                "password: {}\n",
                serde_json::to_string(&password).map_err(|err| AppError::msg(err.to_string()))?
            );
            crate::native::write_secret(Path::new(&secret), &body)?;
            crate::native::mongo_plan(
                &profile.host,
                port,
                &profile.username,
                database,
                &request.path,
                &secret,
                true,
            )
        }
        _ => return Err(AppError::msg("此引擎不支持该原生恢复格式")),
    };
    crate::native::run_plan(&plan)
}

pub fn convert_backup(source: &str, destination: &str) -> AppResult<String> {
    crate::native::convert_dump_to_sql(Path::new(source), Path::new(destination))
}

fn try_native_backup(
    state: &AppState,
    request: &BackupRequest,
    engine: Engine,
) -> AppResult<Option<FileExportResult>> {
    let profile = profile_of(state, &request.connection_id)?;
    if engine == Engine::Mssql {
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
            return Err(AppError::msg(
                failed.error.clone().unwrap_or_else(|| "BACKUP 失败".into()),
            ));
        }
        return Ok(Some(FileExportResult {
            path: request.path.clone(),
            row_count: 0,
            truncated: false,
            kind: "native-backup".into(),
            note: "SQL Server 备份文件位于数据库服务器路径，请验证可还原性。".into(),
        }));
    }
    let tool = match engine {
        Engine::Mysql | Engine::Mariadb => "mysqldump",
        Engine::Postgres => "pg_dump",
        Engine::Mongodb => "mongodump",
        _ => return Ok(None),
    };
    // An unavailable tool can use the explicitly documented auto fallback. A
    // rejected policy or failed backup must never silently produce a weaker artifact.
    if crate::native::tool_path(tool).is_none() {
        return Ok(None);
    }
    validate_native_profile(&profile)?;
    let password = state.decrypt_secret(&profile.password_enc)?;
    let port = if profile.port == 0 {
        engine.default_port()
    } else {
        profile.port
    };
    let database = if request.database.is_empty() {
        &profile.database
    } else {
        &request.database
    };
    let temporary =
        Path::new(&request.path).with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    struct OutputCleanup(std::path::PathBuf);
    impl Drop for OutputCleanup {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let _cleanup = OutputCleanup(temporary.clone());
    let output_path = temporary.display().to_string();
    let plan = match engine {
        Engine::Mysql | Engine::Mariadb => {
            let secret = secret_path(state, "cnf");
            crate::native::write_secret(
                Path::new(&secret),
                &crate::native::mysql_defaults(&profile.host, port, &profile.username, &password),
            )?;
            crate::native::mysql_plan(
                &profile.host,
                port,
                &profile.username,
                database,
                &output_path,
                &secret,
            )
        }
        Engine::Postgres => crate::native::pg_plan(
            &profile.host,
            port,
            &profile.username,
            database,
            &output_path,
            &password,
        ),
        Engine::Mongodb => {
            let secret = secret_path(state, "yaml");
            let body = format!(
                "password: {}\n",
                serde_json::to_string(&password).map_err(|err| AppError::msg(err.to_string()))?
            );
            crate::native::write_secret(Path::new(&secret), &body)?;
            crate::native::mongo_plan(
                &profile.host,
                port,
                &profile.username,
                database,
                &output_path,
                &secret,
                false,
            )
        }
        _ => return Ok(None),
    };
    crate::native::run_plan(&plan)?;
    fs::rename(temporary, &request.path)
        .map_err(|err| AppError::msg(format!("原生备份已生成，但替换目标文件失败：{err}")))?;
    Ok(Some(FileExportResult {
        path: request.path.clone(), row_count: 0, truncated: false, kind: "native-backup".into(),
        note: "原生备份已完成；原生 SQL / 归档将使用对应数据库工具恢复，请在隔离数据库中验证可还原性。".into(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::SaveConnectionRequest;
    use serde_json::json;

    struct Fixture {
        state: AppState,
        directory: std::path::PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let directory =
                std::env::temp_dir().join(format!("dataharbor-backup-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&directory).unwrap();
            let state = AppState::open_in(directory.clone(), true).unwrap();
            Self { state, directory }
        }
        fn database(&self, name: &str) -> String {
            let saved = self
                .state
                .save_connection(SaveConnectionRequest {
                    name: name.into(),
                    engine: Engine::Sqlite,
                    file_path: self
                        .directory
                        .join(format!("{name}.sqlite"))
                        .display()
                        .to_string(),
                    ..SaveConnectionRequest::default()
                })
                .unwrap();
            self.state.connect(&saved.connection.id).unwrap();
            saved.connection.id
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            for profile in self.state.profiles_snapshot() {
                self.state.disconnect(&profile.id);
            }
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    #[test]
    fn native_policy_refuses_tls_and_tunnels_instead_of_downgrading() {
        let mut profile: StoredProfile =
            serde_json::from_value(json!({"id":"fixture", "name":"fixture", "engine":"postgres"}))
                .unwrap();
        validate_native_profile(&profile).unwrap();
        profile.use_ssl = true;
        assert!(validate_native_profile(&profile)
            .unwrap_err()
            .to_string()
            .contains("TLS"));
        profile.use_ssl = false;
        profile.ssh_enabled = true;
        assert!(validate_native_profile(&profile).is_err());
        profile.ssh_enabled = false;
        profile.http_tunnel = true;
        assert!(validate_native_profile(&profile).is_err());
    }

    #[test]
    fn builtin_backup_over_twenty_mb_restores_with_exact_multiline_values() {
        let fixture = Fixture::new();
        let source = fixture.database("source");
        let target = fixture.database("target");
        let payload = format!("前导零00123；引号'；换行\n{}", "x".repeat(90_000));
        fixture
            .state
            .with_session(&source, |session| {
                session.execute(
                    "main",
                    "CREATE TABLE sample (id INTEGER PRIMARY KEY, payload TEXT)",
                    None,
                )?;
                let value = sql_literal(Engine::Sqlite, &JsonValue::String(payload.clone()))?;
                let statements = (0..250)
                    .map(|index| format!("INSERT INTO sample VALUES ({index}, {value})"))
                    .collect::<Vec<_>>();
                session.execute_transaction("main", &statements)
            })
            .unwrap();
        let path = fixture.directory.join("large.sql").display().to_string();
        let result = backup_sql(
            &fixture.state,
            &BackupRequest {
                connection_id: source,
                database: "main".into(),
                schema: String::new(),
                path: path.clone(),
                mode: "builtin".into(),
            },
        )
        .unwrap();
        assert_eq!(result.row_count, 250);
        assert!(fs::metadata(&path).unwrap().len() > 20 * 1024 * 1024);
        restore_sql(
            &fixture.state,
            &RestoreRequest {
                connection_id: target.clone(),
                database: "main".into(),
                path,
            },
        )
        .unwrap();
        fixture
            .state
            .with_session(&target, |session| {
                let count = session.execute("main", "SELECT COUNT(*) FROM sample", Some(1))?;
                assert_eq!(count.rows[0][0], json!(250));
                let actual = session.execute(
                    "main",
                    "SELECT payload FROM sample WHERE id = 249",
                    Some(1),
                )?;
                assert_eq!(actual.rows[0][0], json!(payload));
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn invalid_tail_is_rejected_before_restore_writes() {
        let fixture = Fixture::new();
        let target = fixture.database("target");
        let path = fixture.directory.join("invalid.sql");
        fs::write(
            &path,
            "CREATE TABLE unexpected(id INTEGER); INSERT INTO unexpected VALUES ('unfinished",
        )
        .unwrap();
        assert!(restore_sql(
            &fixture.state,
            &RestoreRequest {
                connection_id: target.clone(),
                database: "main".into(),
                path: path.display().to_string()
            }
        )
        .is_err());
        fixture
            .state
            .with_session(&target, |session| {
                assert!(session.list_tables("main", "")?.is_empty());
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn streamed_sql_keeps_quoted_semicolons_and_utf8_chunk_boundaries() {
        let fixture = Fixture::new();
        let path = fixture.directory.join("quoted.sql");
        let text = format!(
            "-- header\nINSERT INTO x VALUES ('{}');\nSELECT $tag$semicolon;文本$tag$;",
            "字;\n'escaped'".replace('\'', "''").repeat(9000)
        );
        fs::write(&path, &text).unwrap();
        let mut statements = Vec::new();
        visit_sql_file(&path, |sql| {
            statements.push(sql.to_string());
            Ok(())
        })
        .unwrap();
        assert_eq!(statements.len(), 2);
        assert!(statements[0].contains("''escaped''"));
        assert!(statements[1].contains("$tag$semicolon;文本$tag$"));
    }
}
