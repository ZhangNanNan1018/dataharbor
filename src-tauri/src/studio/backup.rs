use super::read_batches::{ReadSession, TableReader};
use super::storage::{engine_of, profile_of};
use super::table_data::ensure_complete;
use super::types::{BackupRequest, FileExportResult, RestoreRequest};
use crate::error::{AppError, AppResult};
use crate::models::{Engine, ExecuteSqlRequest, StoredProfile};
use crate::operations::{AtomicOutput, OperationContext};
use crate::query::execute_sql_with_operation;
use crate::sqlutil::{qualified_name, quote_ident, sql_literal, value_sql};
use crate::state::AppState;
use serde_json::Value as JsonValue;
use std::fs;
use std::io::{Read, Write};
use std::path::Path;

const MAX_STATEMENT_BYTES: usize = 20 * 1024 * 1024;

#[cfg(test)]
pub fn backup_sql(state: &AppState, request: &BackupRequest) -> AppResult<FileExportResult> {
    backup_sql_with_operation(state, request, &OperationContext::detached())
}

pub fn backup_sql_with_operation(
    state: &AppState,
    request: &BackupRequest,
    operation: &OperationContext,
) -> AppResult<FileExportResult> {
    backup_with_ceiling_operation(state, request, None, operation)
}

#[cfg(test)]
pub(super) fn backup_with_ceiling(
    state: &AppState,
    request: &BackupRequest,
    ceiling: u32,
) -> AppResult<FileExportResult> {
    backup_with_ceiling_operation(state, request, Some(ceiling), &OperationContext::detached())
}

fn backup_with_ceiling_operation(
    state: &AppState,
    request: &BackupRequest,
    ceiling: Option<u32>,
    operation: &OperationContext,
) -> AppResult<FileExportResult> {
    operation.checkpoint()?;
    let engine = engine_of(state, &request.connection_id)?;
    let mode = request.mode.to_ascii_lowercase();
    if mode != "builtin" {
        if let Some(result) = try_native_backup(state, request, engine, operation)? {
            return Ok(result);
        }
        if mode == "native" {
            return Err(AppError::msg("已指定只用本机工具，但工具不存在或执行失败。可改成自动模式以回退到内置 INSERT 脚本。"));
        }
    }
    if !engine.is_sql() {
        return Err(AppError::msg(
            "文档库请用 JSON 导出，或安装原生备份工具。逻辑脚本备份目前覆盖 SQL 引擎。",
        ));
    }
    let mut read = ReadSession::open(state, &request.connection_id, &request.database, operation)?;
    let engine = read.session.engine();
    let tables = read
        .session
        .list_tables(&request.database, &request.schema)?;
    let (kind, note) = if engine == Engine::Sqlite {
        ("sqlite-script", "SQLite 表结构和数据脚本；不含独立索引、视图和触发器。全部表来自同一个独立只读快照，有非空主键的表分批写出；其他表保留受限完整读取。")
    } else {
        ("data-only", "仅含 INSERT 数据，目标端必须预先具备兼容表结构。不包含建表、索引、约束、权限等对象，不是完整数据库备份。PostgreSQL/MySQL InnoDB 的非空主键表复用独立快照分批读取；其他路径受限且读取期间需停止写入。")
    };
    let output = AtomicOutput::new(Path::new(&request.path))?;
    let file = fs::File::create(output.path()).map_err(|err| AppError::msg(err.to_string()))?;
    let mut writer = std::io::BufWriter::new(file);
    writeln!(
        writer,
        "-- DataHarbor {kind}\n-- engine: {}\n-- {note}\n",
        engine.label()
    )
    .map_err(|err| AppError::msg(err.to_string()))?;
    let mut count = 0u64;
    for (table_index, table) in tables.iter().enumerate() {
        operation.checkpoint()?;
        operation.report(
            "读取备份表",
            table_index as u64,
            Some(tables.len() as u64),
            &format!("正在读取 {table}；取消不会替换旧备份"),
        );
        let name = qualified_name(engine, &request.database, &request.schema, table)?;
        writeln!(writer, "-- {name}").map_err(|err| AppError::msg(err.to_string()))?;
        if engine == Engine::Sqlite {
            let outcome = read.session.execute(
                &request.database,
                &format!(
                    "SELECT sql FROM sqlite_master WHERE name = {}",
                    sql_literal(Engine::Sqlite, &JsonValue::String(table.clone()))?
                ),
                Some(1),
            )?;
            let definition = outcome
                .rows
                .first()
                .and_then(|row| row.first())
                .and_then(|value| value.as_str())
                .ok_or_else(|| AppError::msg(format!("无法读取 {table} 的建表语句，已取消备份")))?;
            write_statement(&mut writer, definition)?;
        }
        let mut reader = TableReader::new(
            &mut *read.session,
            read.snapshot,
            &request.database,
            &request.schema,
            table,
            ceiling,
            operation,
        )?;
        let cols = reader
            .columns
            .iter()
            .map(|column| quote_ident(engine, &column.name))
            .collect::<AppResult<Vec<_>>>()?
            .join(", ");
        while let Some(rows) = reader.next(operation)? {
            for row in rows {
                operation.checkpoint()?;
                let values = row
                    .iter()
                    .zip(&reader.columns)
                    .map(|(value, column)| value_sql(engine, &column.data_type, value))
                    .collect::<AppResult<Vec<_>>>()?
                    .join(", ");
                write_statement(
                    &mut writer,
                    &format!("INSERT INTO {name} ({cols}) VALUES ({values})"),
                )?;
                count += 1;
            }
        }
        ensure_complete(
            reader.total,
            reader.loaded as usize,
            reader.truncated(),
            &format!("内置备份（表 {table}）"),
        )?;
        writeln!(writer).map_err(|err| AppError::msg(err.to_string()))?;
    }
    writer
        .flush()
        .map_err(|err| AppError::msg(err.to_string()))?;
    drop(writer);
    read.finish()?;
    operation.checkpoint()?;
    output.commit()?;
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

#[cfg(test)]
pub fn restore_sql(state: &AppState, request: &RestoreRequest) -> AppResult<String> {
    restore_sql_with_operation(state, request, &OperationContext::detached())
}

pub fn restore_sql_with_operation(
    state: &AppState,
    request: &RestoreRequest,
    operation: &OperationContext,
) -> AppResult<String> {
    operation.checkpoint()?;
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
            let response = execute_sql_with_operation(
                state,
                ExecuteSqlRequest { session_id: None,
                    connection_id: request.connection_id.clone(),
                    database: request.database.clone(),
                    sql,
                    page: 1,
                    page_size: 5,
                    only_statement: None,
                },
                operation,
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
            operation,
        );
    }
    // Validate every statement before applying the first one. Cancellation during
    // this phase cannot have database side effects.
    let mut total = 0u64;
    visit_sql_file_with_operation(
        Path::new(&request.path),
        operation,
        "预检还原脚本（字节）",
        |_| {
            total += 1;
            Ok(())
        },
    )?;
    operation.checkpoint()?;
    let mut applied = 0u64;
    // Restore has its own connection. Dropping it on cancellation rolls back
    // open transactions without touching transactions in an editor tab.
    let mut session = state.open_isolated_session(&request.connection_id)?;
    let mut transaction = RestoreTransaction::Auto;
    visit_sql_file_with_operation(Path::new(&request.path), operation, "", |sql| {
        operation.report(
            "执行还原（语句）",
            applied,
            Some(total),
            "已执行语句不等于已提交；取消会关闭本次还原连接，已确认提交之外的复杂脚本结果仍需核对",
        );
        operation.checkpoint()?;
        let interrupt = operation.bind_interrupt(session.cancel_handle(&request.database)?)?;
        let result = session.execute(&request.database, sql, Some(0));
        drop(interrupt);
        if let Err(error) = result {
            if operation.cancel_requested() {
                return Err(operation.cancel_error());
            }
            return Err(AppError::msg(format!("还原在第 {} 条语句失败；前 {applied} 条已执行，未提交事务将随连接关闭回滚，已提交部分需核对：{error}", applied + 1)));
        }
        applied += 1;
        if let Some(committed) = transaction.observe(sql, engine) {
            if committed > 0 {
                operation.committed(committed);
            }
        }
        operation.report(
            "执行还原（语句）",
            applied,
            Some(total),
            "已提交量按可确认提交的写语句计；读取和未提交语句不计入，复杂脚本副作用需核对",
        );
        operation.checkpoint()
    })?;
    Ok(format!("已执行 {applied} 条语句"))
}

#[derive(Debug)]
enum RestoreTransaction {
    Auto,
    Open(u64),
    Unknown,
}
impl RestoreTransaction {
    fn observe(&mut self, sql: &str, engine: Engine) -> Option<u64> {
        let head = crate::sqlutil::strip_leading_comments(sql).to_ascii_uppercase();
        let words = head.split_whitespace().collect::<Vec<_>>();
        let first = words.first().copied().unwrap_or("");
        if matches!(self, Self::Unknown) {
            return None;
        }
        if first == "BEGIN"
            && words
                .get(1)
                .is_none_or(|word| matches!(*word, "TRAN" | "TRANSACTION" | "WORK"))
            || first == "START" && words.get(1) == Some(&"TRANSACTION")
        {
            // Nested BEGIN differs across engines (warning, implicit commit,
            // or transaction depth). Do not guess which writes became durable.
            *self = if matches!(self, Self::Auto) {
                Self::Open(0)
            } else {
                Self::Unknown
            };
            return None;
        }
        if matches!(first, "COMMIT" | "END") {
            if words.contains(&"PREPARED") {
                *self = Self::Unknown;
                return None;
            }
            let completed = match self {
                Self::Open(count) => Some(*count),
                _ => None,
            };
            *self = if words.contains(&"CHAIN") && !words.contains(&"NO") {
                Self::Open(0)
            } else {
                Self::Auto
            };
            return completed;
        }
        if first == "ROLLBACK" {
            *self = if words.iter().skip(1).any(|word| {
                !matches!(
                    *word,
                    "WORK" | "TRAN" | "TRANSACTION" | "AND" | "NO" | "CHAIN"
                )
            }) {
                Self::Unknown
            } else if words.contains(&"CHAIN") && !words.contains(&"NO") {
                Self::Open(0)
            } else {
                Self::Auto
            };
            return None;
        }
        let ddl = matches!(
            first,
            "CREATE" | "ALTER" | "DROP" | "TRUNCATE" | "GRANT" | "REVOKE"
        );
        if matches!(
            first,
            "SET" | "SAVE" | "SAVEPOINT" | "RELEASE" | "CALL" | "EXEC" | "EXECUTE" | "DO" | "BEGIN"
        ) || ddl
            && matches!(
                engine,
                Engine::Mysql | Engine::Mariadb | Engine::Oracle | Engine::Dameng
            )
            && matches!(self, Self::Open(_))
        {
            // Session modes, procedures and implicit-commit DDL cannot be
            // accounted for reliably by a lightweight script dispatcher.
            *self = Self::Unknown;
            return None;
        }
        let mutation = ddl || matches!(first, "INSERT" | "UPDATE" | "DELETE" | "REPLACE" | "MERGE");
        if !mutation {
            return None;
        }
        match self {
            Self::Auto => Some(1),
            Self::Open(pending) => {
                *pending += 1;
                None
            }
            Self::Unknown => None,
        }
    }
}

#[cfg(test)]
fn visit_sql_file(path: &Path, visit: impl FnMut(&str) -> AppResult<()>) -> AppResult<()> {
    visit_sql_file_with_operation(path, &OperationContext::detached(), "", visit)
}

fn visit_sql_file_with_operation(
    path: &Path,
    operation: &OperationContext,
    phase: &str,
    mut visit: impl FnMut(&str) -> AppResult<()>,
) -> AppResult<()> {
    operation.checkpoint()?;
    let total_bytes = fs::metadata(path)
        .map_err(|err| AppError::msg(err.to_string()))?
        .len();
    let mut completed_bytes = 0u64;
    let mut file = fs::File::open(path).map_err(|err| AppError::msg(err.to_string()))?;
    let mut pending = Vec::new();
    let mut chunk = [0; 64 * 1024];
    loop {
        operation.checkpoint()?;
        let read = file
            .read(&mut chunk)
            .map_err(|err| AppError::msg(err.to_string()))?;
        completed_bytes += read as u64;
        if !phase.is_empty() {
            operation.report(
                phase,
                completed_bytes,
                Some(total_bytes),
                "完整校验完成前不会执行语句",
            );
        }
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
                operation.checkpoint()?;
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
                operation.checkpoint()?;
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
    operation: &OperationContext,
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
                crate::native::prepare_pg_script_with_operation(
                    Path::new(&request.path),
                    Path::new(&script),
                    operation,
                )?;
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
    crate::native::run_plan_with_operation(&plan, operation)
}

pub fn convert_backup_with_operation(
    source: &str,
    destination: &str,
    operation: &OperationContext,
) -> AppResult<String> {
    crate::native::convert_dump_to_sql_with_operation(
        Path::new(source),
        Path::new(destination),
        operation,
    )
}

fn try_native_backup(
    state: &AppState,
    request: &BackupRequest,
    engine: Engine,
    operation: &OperationContext,
) -> AppResult<Option<FileExportResult>> {
    operation.checkpoint()?;
    let profile = profile_of(state, &request.connection_id)?;
    if engine == Engine::Mssql {
        let sql = crate::native::mssql_backup_sql(&request.database, &request.path);
        let response = execute_sql_with_operation(
            state,
            ExecuteSqlRequest { session_id: None,
                connection_id: request.connection_id.clone(),
                database: request.database.clone(),
                sql,
                page: 1,
                page_size: 5,
                only_statement: None,
            },
            operation,
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
    crate::native::run_plan_with_operation(&plan, operation)?;
    operation.checkpoint()?;
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

    #[test]
    fn precancelled_backup_does_not_replace_existing_artifact() {
        let fixture = Fixture::new();
        let id = fixture.database("source");
        let destination = fixture.directory.join("existing.sql");
        fs::write(&destination, "previous backup").unwrap();
        let operation = OperationContext::detached();
        operation.request_cancel().unwrap();
        let result = backup_sql_with_operation(
            &fixture.state,
            &BackupRequest {
                connection_id: id,
                database: "main".into(),
                schema: String::new(),
                path: destination.display().to_string(),
                mode: "builtin".into(),
            },
            &operation,
        );
        assert!(matches!(result, Err(AppError::Cancelled { .. })));
        assert_eq!(fs::read_to_string(destination).unwrap(), "previous backup");
        assert!(fs::read_dir(&fixture.directory).unwrap().all(|entry| entry
            .unwrap()
            .path()
            .extension()
            .is_none_or(|extension| extension != "tmp")));
    }

    #[test]
    fn cancelled_restore_preserves_completed_statements_and_skips_later_writes() {
        let fixture = Fixture::new();
        let id = fixture.database("target");
        let path = fixture.directory.join("cancel.sql");
        fs::write(&path, "-- DataHarbor fixture\nCREATE TABLE items(id INTEGER PRIMARY KEY); INSERT INTO items VALUES(1); WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<100000000) SELECT sum(x) FROM n; INSERT INTO items VALUES(2);").unwrap();
        let operation = OperationContext::detached();
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                restore_sql_with_operation(
                    &fixture.state,
                    &RestoreRequest {
                        connection_id: id.clone(),
                        database: "main".into(),
                        path: path.display().to_string(),
                    },
                    &operation,
                )
            });
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                let progress = operation.snapshot();
                if progress.committed == 2 && progress.interruptible {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "restore did not reach the long statement"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            operation.request_cancel().unwrap();
            assert!(matches!(
                worker.join().unwrap(),
                Err(AppError::Cancelled {
                    committed: 2,
                    batches: 2
                })
            ));
        });
        let rows = fixture
            .state
            .with_session(&id, |session| {
                session.execute("main", "SELECT id FROM items ORDER BY id", None)
            })
            .unwrap()
            .rows;
        assert_eq!(rows, vec![vec![json!(1)]]);
    }

    #[test]
    fn cancelled_transactional_restore_rolls_back_only_its_own_connection() {
        let fixture = Fixture::new();
        let id = fixture.database("transactional-target");
        let path = fixture.directory.join("transactional.sql");
        fs::write(&path, "-- DataHarbor fixture\nBEGIN; CREATE TABLE uncommitted(id INTEGER PRIMARY KEY); INSERT INTO uncommitted VALUES(1); WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<100000000) SELECT sum(x) FROM n; COMMIT;").unwrap();
        let operation = OperationContext::detached();
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                restore_sql_with_operation(
                    &fixture.state,
                    &RestoreRequest {
                        connection_id: id.clone(),
                        database: "main".into(),
                        path: path.display().to_string(),
                    },
                    &operation,
                )
            });
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                let progress = operation.snapshot();
                if progress.phase == "执行还原（语句）"
                    && progress.completed == 3
                    && progress.interruptible
                {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "restore did not reach the transactional long statement"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            operation.request_cancel().unwrap();
            assert!(matches!(
                worker.join().unwrap(),
                Err(AppError::Cancelled {
                    committed: 0,
                    batches: 0
                })
            ));
        });
        let tables = fixture
            .state
            .with_session(&id, |session| session.list_tables("main", ""))
            .unwrap();
        assert!(
            tables.is_empty(),
            "the dedicated connection must roll back unfinished DDL"
        );
    }

    #[test]
    fn restore_progress_only_counts_confirmed_commits() {
        let mut tracking = RestoreTransaction::Auto;
        assert_eq!(tracking.observe("BEGIN", Engine::Postgres), None);
        assert_eq!(
            tracking.observe("INSERT INTO items VALUES(1)", Engine::Postgres),
            None
        );
        assert_eq!(tracking.observe("SELECT 1", Engine::Postgres), None);
        assert_eq!(tracking.observe("COMMIT", Engine::Postgres), Some(1));
        assert_eq!(tracking.observe("BEGIN", Engine::Postgres), None);
        assert_eq!(
            tracking.observe("DELETE FROM items", Engine::Postgres),
            None
        );
        assert_eq!(tracking.observe("ROLLBACK", Engine::Postgres), None);
        assert_eq!(tracking.observe("SET autocommit=0", Engine::Mysql), None);
        assert_eq!(
            tracking.observe("INSERT INTO items VALUES(2)", Engine::Mysql),
            None
        );
        assert_eq!(tracking.observe("COMMIT", Engine::Mysql), None);
        let mut chained = RestoreTransaction::Auto;
        chained.observe("BEGIN", Engine::Postgres);
        chained.observe("INSERT INTO items VALUES(1)", Engine::Postgres);
        assert_eq!(
            chained.observe("COMMIT AND CHAIN", Engine::Postgres),
            Some(1)
        );
        assert_eq!(
            chained.observe("INSERT INTO items VALUES(2)", Engine::Postgres),
            None
        );
        assert_eq!(chained.observe("ROLLBACK", Engine::Postgres), None);
        let mut nested = RestoreTransaction::Auto;
        nested.observe("BEGIN TRANSACTION", Engine::Mssql);
        nested.observe("BEGIN TRANSACTION", Engine::Mssql);
        nested.observe("INSERT INTO items VALUES(1)", Engine::Mssql);
        assert_eq!(nested.observe("COMMIT", Engine::Mssql), None);
    }
}
