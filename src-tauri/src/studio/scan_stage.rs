//! Bounded keyset reads and a private on-disk staging index. No target writes occur here.
use super::super::key_family;
use crate::drivers::DbSession;
use crate::error::{AppError, AppResult};
use crate::models::{ColumnMeta, Engine, RowChange};
use crate::operations::OperationContext;
use crate::sqlutil::{binary_cell_bytes, json_count, qualified_name, quote_ident, value_sql};
use crate::studio::storage::cell_string;
use crate::studio::table_data::BATCH;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

#[path = "sync_key_semantics.rs"]
mod semantics;

#[derive(Clone, Copy)]
pub(super) enum Side {
    Source,
    Target,
}
impl Side {
    fn table(self) -> &'static str {
        match self {
            Self::Source => "source_rows",
            Self::Target => "target_rows",
        }
    }
}

struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(super) struct Stage {
    // Field order closes SQLite before removing its directory on Windows.
    db: Connection,
    _directory: Directory,
    pub(super) max_batch: usize,
}

pub(super) struct Relation<'a> {
    pub database: &'a str,
    pub schema: &'a str,
    pub table: &'a str,
    pub columns: &'a [ColumnMeta],
    pub canonical_keys: &'a [String],
}

impl Stage {
    pub(super) fn new(config: &Path) -> AppResult<Self> {
        let parent = config.join("operations");
        fs::create_dir_all(&parent).map_err(db_error)?;
        let directory = parent.join(format!("sync-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&directory).map_err(db_error)?;
        let cleanup = Directory(directory.clone());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).map_err(db_error)?;
        }
        let path = directory.join("stage.sqlite");
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        drop(options.open(&path).map_err(db_error)?);
        let db = Connection::open(path).map_err(db_error)?;
        // All queries below use a primary-key range and LIMIT. There is no
        // unbounded sort or temporary table, and the page cache is 4 MiB.
        db.execute_batch("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; PRAGMA cache_size=-4096; PRAGMA mmap_size=0; PRAGMA temp_store=MEMORY;
            CREATE TABLE source_rows(key TEXT PRIMARY KEY, row_json TEXT NOT NULL) WITHOUT ROWID;
            CREATE TABLE target_rows(key TEXT PRIMARY KEY, row_json TEXT NOT NULL) WITHOUT ROWID;
            CREATE TABLE changes(id INTEGER PRIMARY KEY, change_json TEXT NOT NULL);").map_err(db_error)?;
        Ok(Self {
            db,
            _directory: cleanup,
            max_batch: 0,
        })
    }

    pub(super) fn scan(
        &mut self,
        side: Side,
        session: &mut dyn DbSession,
        relation: &Relation<'_>,
        operation: &OperationContext,
        phase: &str,
        after_page: &mut dyn FnMut(u64),
    ) -> AppResult<u64> {
        operation.checkpoint()?;
        let interrupt = operation.bind_interrupt(session.cancel_handle(relation.database)?)?;
        session
            .begin_read_snapshot(relation.database)
            .map_err(|error| {
                if operation.cancel_requested() {
                    operation.cancel_error()
                } else {
                    error
                }
            })?;
        let result = self.read_snapshot(side, session, relation, operation, phase, after_page);
        // Never let a late cancellation interrupt ROLLBACK or the next operation.
        drop(interrupt);
        let ended = session.end_read_snapshot(relation.database);
        operation.checkpoint()?;
        match result {
            Ok(total) => {
                ended?;
                Ok(total)
            }
            Err(error) => Err(error),
        }
    }

    fn read_snapshot(
        &mut self,
        side: Side,
        session: &mut dyn DbSession,
        relation: &Relation<'_>,
        operation: &OperationContext,
        phase: &str,
        after_page: &mut dyn FnMut(u64),
    ) -> AppResult<u64> {
        operation.checkpoint()?;
        session.validate_snapshot_relation(relation.database, relation.schema, relation.table)?;
        operation.checkpoint()?;
        let columns = session.list_columns(relation.database, relation.schema, relation.table)?;
        if serde_json::to_value(&columns).map_err(db_error)?
            != serde_json::to_value(relation.columns).map_err(db_error)?
        {
            return Err(AppError::msg(
                "读取快照前表结构发生变化，已取消；没有开始写入目标",
            ));
        }
        operation.checkpoint()?;
        semantics::validate(session, relation)?;
        operation.checkpoint()?;
        let name = qualified_name(
            session.engine(),
            relation.database,
            relation.schema,
            relation.table,
        )?;
        let count = session.execute(
            relation.database,
            &format!("SELECT COUNT(*) FROM {name}"),
            Some(1),
        )?;
        let total = count
            .rows
            .first()
            .and_then(|row| row.first())
            .and_then(json_count)
            .ok_or_else(|| AppError::msg("无法确认快照行数，已停止；没有开始写入目标"))?;
        let keys = key_indices(&columns)?;
        let mut last: Option<Vec<Value>> = None;
        let mut loaded = 0;
        operation.report(
            phase,
            0,
            Some(total),
            "正在读取一致性快照并建立临时磁盘索引；尚未写入目标",
        );
        loop {
            operation.checkpoint()?;
            let sql = keyset_sql(
                session.engine(),
                &name,
                &columns,
                &keys,
                last.as_deref(),
                BATCH,
            )?;
            let page = session.execute(relation.database, &sql, Some(BATCH as usize))?;
            if page.truncated
                || page.rows.len() > BATCH as usize
                || page.rows.iter().any(|row| row.len() != columns.len())
            {
                return Err(AppError::msg(
                    "快照读取被截断或列数不匹配，已取消；没有开始写入目标",
                ));
            }
            if !page.columns.is_empty()
                && page
                    .columns
                    .iter()
                    .map(|c| &c.name)
                    .ne(columns.iter().map(|c| &c.name))
            {
                return Err(AppError::msg(
                    "快照查询列顺序改变，已取消；没有开始写入目标",
                ));
            }
            let length = page.rows.len();
            self.max_batch = self.max_batch.max(length);
            self.db.execute_batch("BEGIN").map_err(db_error)?;
            let insert_sql = format!("INSERT INTO {}(key,row_json) VALUES(?1,?2)", side.table());
            let result = (|| {
                let mut insert = self.db.prepare_cached(&insert_sql).map_err(db_error)?;
                for row in &page.rows {
                    operation.checkpoint()?;
                    let key = canonical_key(&columns, row, relation.canonical_keys)?;
                    let row_json = serde_json::to_string(row).map_err(db_error)?;
                    insert
                        .execute(params![key, row_json])
                        .map_err(|error| match error {
                            rusqlite::Error::SqliteFailure(ref failure, _)
                                if failure.code == rusqlite::ErrorCode::ConstraintViolation =>
                            {
                                AppError::msg(
                                    "检测到重复或语义冲突的主键，已取消；没有开始写入目标",
                                )
                            }
                            other => db_error(other),
                        })?;
                }
                Ok(())
            })();
            match result {
                Ok(()) => self.db.execute_batch("COMMIT").map_err(db_error)?,
                Err(error) => {
                    let _ = self.db.execute_batch("ROLLBACK");
                    return Err(error);
                }
            }
            loaded += length as u64;
            if loaded > total {
                return Err(AppError::msg(
                    "读取行数超过快照总数，已取消；没有开始写入目标",
                ));
            }
            operation.report(
                phase,
                loaded,
                Some(total),
                "正在读取一致性快照并建立临时磁盘索引；尚未写入目标",
            );
            after_page(loaded);
            operation.checkpoint()?;
            if let Some(row) = page.rows.last() {
                last = Some(keys.iter().map(|&index| row[index].clone()).collect());
            }
            if length < BATCH as usize || loaded == total {
                break;
            }
        }
        if loaded != total {
            return Err(AppError::msg(format!(
                "未完整读取快照：预期 {total} 行，实际 {loaded} 行；没有开始写入目标"
            )));
        }
        Ok(total)
    }

    pub(super) fn input_page(
        &mut self,
        side: Side,
        after: &str,
    ) -> AppResult<Vec<(String, Vec<Value>)>> {
        let sql = format!(
            "SELECT key,row_json FROM {} WHERE key>?1 ORDER BY key LIMIT ?2",
            side.table()
        );
        let mut statement = self.db.prepare_cached(&sql).map_err(db_error)?;
        let mut cursor = statement.query(params![after, BATCH]).map_err(db_error)?;
        let mut rows = Vec::new();
        while let Some(row) = cursor.next().map_err(db_error)? {
            let text: String = row.get(1).map_err(db_error)?;
            rows.push((
                row.get(0).map_err(db_error)?,
                serde_json::from_str(&text).map_err(db_error)?,
            ));
        }
        self.max_batch = self.max_batch.max(rows.len());
        Ok(rows)
    }

    pub(super) fn get(&self, side: Side, key: &str) -> AppResult<Option<Vec<Value>>> {
        let sql = format!("SELECT row_json FROM {} WHERE key=?1", side.table());
        let mut statement = self.db.prepare_cached(&sql).map_err(db_error)?;
        let text: Option<String> = statement
            .query_row([key], |row| row.get(0))
            .optional()
            .map_err(db_error)?;
        text.map(|value| serde_json::from_str(&value).map_err(db_error))
            .transpose()
    }

    pub(super) fn contains_source(&self, key: &str) -> AppResult<bool> {
        self.db
            .prepare_cached("SELECT EXISTS(SELECT 1 FROM source_rows WHERE key=?1)")
            .map_err(db_error)?
            .query_row([key], |row| row.get(0))
            .map_err(db_error)
    }

    pub(super) fn begin_plan_batch(&self) -> AppResult<()> {
        self.db.execute_batch("BEGIN").map_err(db_error)
    }
    pub(super) fn end_plan_batch(&self) -> AppResult<()> {
        self.db.execute_batch("COMMIT").map_err(db_error)
    }

    pub(super) fn append_change(&self, change: &RowChange) -> AppResult<()> {
        #[derive(serde::Serialize)]
        struct StoredChange<'a> {
            kind: &'a str,
            pk: &'a [crate::models::FieldValue],
            values: &'a [crate::models::FieldValue],
            original: &'a Option<Vec<crate::models::FieldValue>>,
        }
        let value = StoredChange {
            kind: &change.kind,
            pk: &change.pk,
            values: &change.values,
            original: &change.original,
        };
        let text = serde_json::to_string(&value).map_err(db_error)?;
        self.db
            .prepare_cached("INSERT INTO changes(change_json) VALUES(?1)")
            .map_err(db_error)?
            .execute([text])
            .map_err(db_error)?;
        Ok(())
    }

    pub(super) fn changes_page(&mut self, after: i64) -> AppResult<Vec<(i64, RowChange)>> {
        let mut statement = self
            .db
            .prepare_cached("SELECT id,change_json FROM changes WHERE id>?1 ORDER BY id LIMIT ?2")
            .map_err(db_error)?;
        let mut cursor = statement.query(params![after, BATCH]).map_err(db_error)?;
        let mut changes = Vec::new();
        while let Some(row) = cursor.next().map_err(db_error)? {
            let text: String = row.get(1).map_err(db_error)?;
            changes.push((
                row.get(0).map_err(db_error)?,
                serde_json::from_str(&text).map_err(db_error)?,
            ));
        }
        self.max_batch = self.max_batch.max(changes.len());
        Ok(changes)
    }
}

fn db_error(error: impl std::fmt::Display) -> AppError {
    AppError::msg(format!("临时同步索引失败：{error}"))
}

fn key_indices(columns: &[ColumnMeta]) -> AppResult<Vec<usize>> {
    let indices = columns
        .iter()
        .enumerate()
        .filter(|(_, column)| column.primary_key)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if indices.is_empty() {
        return Err(AppError::msg("流式读取需要真实完整主键；没有开始写入目标"));
    }
    Ok(indices)
}

fn keyset_sql(
    engine: Engine,
    name: &str,
    columns: &[ColumnMeta],
    keys: &[usize],
    last: Option<&[Value]>,
    limit: u32,
) -> AppResult<String> {
    let projection = columns
        .iter()
        .map(|column| quote_ident(engine, &column.name))
        .collect::<AppResult<Vec<_>>>()?
        .join(", ");
    let names = keys
        .iter()
        .map(|&index| {
            let quoted = quote_ident(engine, &columns[index].name)?;
            Ok(
                if engine == Engine::Sqlite && key_family(&columns[index].data_type) == "text" {
                    format!("{quoted} COLLATE BINARY")
                } else {
                    quoted
                },
            )
        })
        .collect::<AppResult<Vec<_>>>()?;
    let predicate = if let Some(values) = last {
        if values.len() != keys.len() || values.iter().any(Value::is_null) {
            return Err(AppError::msg("主键游标无效；没有开始写入目标"));
        }
        let literals = keys
            .iter()
            .zip(values)
            .map(|(&index, value)| value_sql(engine, &columns[index].data_type, value))
            .collect::<AppResult<Vec<_>>>()?;
        let alternatives = (0..keys.len())
            .map(|index| {
                let mut parts = (0..index)
                    .map(|previous| format!("{} = {}", names[previous], literals[previous]))
                    .collect::<Vec<_>>();
                parts.push(format!("{} > {}", names[index], literals[index]));
                format!("({})", parts.join(" AND "))
            })
            .collect::<Vec<_>>();
        format!(" WHERE {}", alternatives.join(" OR "))
    } else {
        String::new()
    };
    Ok(format!(
        "SELECT {projection} FROM {name}{predicate} ORDER BY {} LIMIT {limit}",
        names.join(", ")
    ))
}

pub(super) fn canonical_key(
    columns: &[ColumnMeta],
    row: &[Value],
    names: &[String],
) -> AppResult<String> {
    let values = names
        .iter()
        .map(|name| {
            let index = columns
                .iter()
                .position(|column| &column.name == name)
                .ok_or_else(|| AppError::msg(format!("缺少主键列 {name}")))?;
            let value = row
                .get(index)
                .filter(|value| !value.is_null())
                .ok_or_else(|| {
                    AppError::msg(format!("主键列 {name} 存在空值或缺失；没有开始写入目标"))
                })?;
            canonical_value(&columns[index], value)
        })
        .collect::<AppResult<Vec<_>>>()?;
    serde_json::to_string(&values).map_err(db_error)
}

pub(super) fn canonical_value(column: &ColumnMeta, value: &Value) -> AppResult<Value> {
    if value.is_null() {
        return Ok(Value::Null);
    }
    Ok(match key_family(&column.data_type) {
        "number" => json!({"number": canonical_number(&cell_string(Some(value)))?}),
        "bool" => match cell_string(Some(value)).to_ascii_lowercase().as_str() {
            "true" | "t" | "1" => json!({"bool":true}),
            "false" | "f" | "0" => json!({"bool":false}),
            _ => return Err(AppError::msg("布尔主键值无法无损规范化")),
        },
        "binary" => {
            json!({"binary": binary_cell_bytes(value).ok_or_else(|| AppError::msg("二进制主键值无法无损规范化"))?})
        }
        _ => value.clone(),
    })
}

fn canonical_number(text: &str) -> AppResult<String> {
    let checked = crate::sqlutil::exact_numeric_literal(text)?;
    let (negative, unsigned) = if let Some(value) = checked.strip_prefix('-') {
        (true, value)
    } else {
        (false, checked.strip_prefix('+').unwrap_or(&checked))
    };
    let (mantissa, exponent) = unsigned
        .split_once(['e', 'E'])
        .map(|(m, e)| (m, e.parse::<i64>()))
        .unwrap_or((unsigned, Ok(0)));
    let exponent = exponent.map_err(|_| AppError::msg("数值主键的指数超出支持范围"))?;
    let fraction = mantissa
        .split_once('.')
        .map(|(_, tail)| tail.len())
        .unwrap_or(0);
    let digits = mantissa.replace('.', "");
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        return Ok("0".into());
    }
    let trimmed = digits.trim_end_matches('0');
    let exponent = exponent
        .checked_sub(fraction as i64)
        .and_then(|n| n.checked_add((digits.len() - trimmed.len()) as i64))
        .ok_or_else(|| AppError::msg("数值主键的指数超出支持范围"))?;
    Ok(format!(
        "{}{trimmed}e{exponent}",
        if negative { "-" } else { "" }
    ))
}

#[cfg(test)]
#[path = "scan_stage_tests.rs"]
mod tests;
