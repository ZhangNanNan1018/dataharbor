use std::fs;
use std::path::Path;

use rusqlite::types::ValueRef;
use rusqlite::Connection;
use serde_json::Value as JsonValue;

use super::DbSession;
use crate::error::{AppError, AppResult};
use crate::models::{ColumnMeta, Engine, QueryOutcome, SessionConfig};
use crate::sqlutil::{json_blob, json_f64, json_i64};

pub struct SqliteSession {
    conn: Connection,
}

pub fn connect(config: &SessionConfig) -> AppResult<SqliteSession> {
    let path = config.file_path.trim();
    if path.is_empty() {
        return Err(AppError::msg("请选择 SQLite 数据库文件"));
    }
    if let Some(parent) = Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|err| AppError::msg(format!("无法创建目录: {err}")))?;
        }
    }
    let conn = Connection::open(path).map_err(|err| AppError::msg(err.to_string()))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|err| AppError::msg(err.to_string()))?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|err| AppError::msg(err.to_string()))?;
    Ok(SqliteSession { conn })
}

impl DbSession for SqliteSession {
    fn engine(&self) -> Engine {
        Engine::Sqlite
    }

    fn cancel_handle(
        &mut self,
        _database: &str,
    ) -> AppResult<Option<crate::operations::CancelHandle>> {
        let interrupt = self.conn.get_interrupt_handle();
        Ok(Some(std::sync::Arc::new(move || {
            interrupt.interrupt();
            Ok(())
        })))
    }

    fn begin_read_snapshot(&mut self, _database: &str) -> AppResult<()> {
        self.conn
            .execute_batch("PRAGMA query_only = ON; BEGIN; SELECT name FROM sqlite_schema LIMIT 1;")
            .map_err(|err| AppError::msg(format!("无法建立 SQLite 只读快照：{err}")))
    }

    fn end_read_snapshot(&mut self, _database: &str) -> AppResult<()> {
        let result = self.conn.execute_batch("ROLLBACK;");
        let reset = self.conn.execute_batch("PRAGMA query_only = OFF;");
        result
            .and(reset)
            .map_err(|err| AppError::msg(format!("无法结束 SQLite 快照：{err}")))
    }

    fn version(&mut self) -> AppResult<String> {
        self.conn
            .query_row("SELECT sqlite_version()", [], |row| row.get::<_, String>(0))
            .map_err(|err| AppError::msg(err.to_string()))
    }

    fn list_databases(&mut self) -> AppResult<Vec<String>> {
        Ok(vec!["main".to_string()])
    }

    fn list_schemas(&mut self, _database: &str) -> AppResult<Vec<String>> {
        Ok(vec!["main".to_string()])
    }

    fn list_tables(&mut self, _database: &str, _schema: &str) -> AppResult<Vec<String>> {
        self.query_strings(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
    }

    fn list_views(&mut self, _database: &str, _schema: &str) -> AppResult<Vec<String>> {
        self.query_strings("SELECT name FROM sqlite_master WHERE type = 'view' ORDER BY name")
    }

    fn list_columns(
        &mut self,
        _database: &str,
        _schema: &str,
        object_name: &str,
    ) -> AppResult<Vec<ColumnMeta>> {
        let create_sql: Option<String> = self
            .conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name = ?1",
                [object_name],
                |row| row.get(0),
            )
            .ok();
        let marked_auto = create_sql
            .as_deref()
            .map(|sql| sql.to_ascii_uppercase().contains("AUTOINCREMENT"))
            .unwrap_or(false);
        let pragma = format!(
            "SELECT name, type, \"notnull\", dflt_value, pk FROM pragma_table_info('{}')",
            object_name.replace('\'', "''")
        );
        let mut stmt = self
            .conn
            .prepare(&pragma)
            .map_err(|err| AppError::msg(err.to_string()))?;
        let mut rows = stmt
            .query([])
            .map_err(|err| AppError::msg(err.to_string()))?;
        let mut columns = Vec::new();
        while let Some(row) = rows.next().map_err(|err| AppError::msg(err.to_string()))? {
            let name: String = row.get(0).map_err(|err| AppError::msg(err.to_string()))?;
            let data_type: String = row
                .get::<_, Option<String>>(1)
                .unwrap_or(None)
                .unwrap_or_default();
            let not_null: i64 = row.get(2).unwrap_or(0);
            let default_value: Option<String> = row.get(3).unwrap_or(None);
            let pk: i64 = row.get(4).unwrap_or(0);
            columns.push(ColumnMeta {
                name,
                data_type,
                nullable: not_null == 0,
                primary_key: pk > 0,
                auto_increment: false,
                default_value,
            });
        }
        let pk_count = columns.iter().filter(|column| column.primary_key).count();
        if pk_count == 1 {
            if let Some(column) = columns.iter_mut().find(|column| column.primary_key) {
                if marked_auto || column.data_type.eq_ignore_ascii_case("INTEGER") {
                    column.auto_increment = true;
                }
            }
        }
        Ok(columns)
    }

    fn relation_kind(
        &mut self,
        _database: &str,
        _schema: &str,
        object_name: &str,
    ) -> AppResult<Option<String>> {
        let kind: Option<String> = self
            .conn
            .query_row(
                "SELECT type FROM sqlite_master WHERE name = ?1 AND type IN ('table', 'view')",
                [object_name],
                |row| row.get(0),
            )
            .ok();
        Ok(kind)
    }

    fn execute(
        &mut self,
        _database: &str,
        sql: &str,
        max_rows: Option<usize>,
    ) -> AppResult<QueryOutcome> {
        let sql = sql.trim();
        if sql.is_empty() {
            return Ok(QueryOutcome::empty());
        }
        let mut stmt = match self.conn.prepare(sql) {
            Ok(stmt) => stmt,
            Err(_) => {
                self.conn
                    .execute_batch(sql)
                    .map_err(|err| AppError::msg(err.to_string()))?;
                let mut outcome = QueryOutcome::empty();
                outcome.rows_affected = self.conn.changes();
                return Ok(outcome);
            }
        };
        let column_count = stmt.column_count();
        if column_count == 0 {
            let affected = stmt
                .execute([])
                .map_err(|err| AppError::msg(err.to_string()))?;
            let mut outcome = QueryOutcome::empty();
            outcome.rows_affected = affected as u64;
            return Ok(outcome);
        }
        let mut columns = Vec::with_capacity(column_count);
        for index in 0..column_count {
            let name = stmt
                .column_name(index)
                .map_err(|err| AppError::msg(err.to_string()))?
                .to_string();
            columns.push(ColumnMeta::named(name));
        }
        let mut cursor = stmt
            .query([])
            .map_err(|err| AppError::msg(err.to_string()))?;
        let mut rows = Vec::new();
        let mut truncated = false;
        while let Some(row) = cursor
            .next()
            .map_err(|err| AppError::msg(err.to_string()))?
        {
            if max_rows.is_some_and(|limit| rows.len() >= limit) {
                truncated = true;
                break;
            }
            let mut cells = Vec::with_capacity(column_count);
            for index in 0..column_count {
                let value = row
                    .get_ref(index)
                    .map_err(|err| AppError::msg(err.to_string()))?;
                cells.push(match value {
                    ValueRef::Null => JsonValue::Null,
                    ValueRef::Integer(number) => json_i64(number),
                    ValueRef::Real(number) => json_f64(number),
                    ValueRef::Text(bytes) => {
                        JsonValue::String(String::from_utf8_lossy(bytes).into_owned())
                    }
                    ValueRef::Blob(bytes) => json_blob(bytes),
                });
            }
            rows.push(cells);
        }
        Ok(QueryOutcome {
            original_rows: None,
            columns,
            rows,
            rows_affected: 0,
            truncated,
        })
    }
}

impl SqliteSession {
    fn query_strings(&self, sql: &str) -> AppResult<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare(sql)
            .map_err(|err| AppError::msg(err.to_string()))?;
        let mut rows = stmt
            .query([])
            .map_err(|err| AppError::msg(err.to_string()))?;
        let mut names = Vec::new();
        while let Some(row) = rows.next().map_err(|err| AppError::msg(err.to_string()))? {
            names.push(row.get(0).map_err(|err| AppError::msg(err.to_string()))?);
        }
        Ok(names)
    }
}
