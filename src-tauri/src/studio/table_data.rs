use super::read_batches::{ReadSession, TableReader};
use super::storage::engine_of;
use super::types::{GenerateRequest, MaskRequest};
use crate::catalogdiff;
use crate::data::commit_changes;
use crate::error::{AppError, AppResult};
use crate::models::{ColumnMeta, CommitRequest, Engine, ExecuteSqlRequest, FieldValue, RowChange};
use crate::operations::OperationContext;
use crate::query::execute_sql;
use crate::sqlutil::{qualified_name, quote_ident};
use crate::state::AppState;
use serde_json::{json, Value as JsonValue};

pub(super) const BATCH: u32 = 500;
pub(super) const CEILING: u32 = 200_000;

pub fn generate_rows(state: &AppState, request: &GenerateRequest) -> AppResult<usize> {
    let count = request.count.clamp(1, 500);
    let columns = columns_of(
        state,
        &request.connection_id,
        &request.database,
        &request.schema,
        &request.table,
    )?;
    let mut changes = Vec::new();
    for index in 1..=count {
        let values = columns
            .iter()
            .filter(|column| !column.auto_increment)
            .map(|column| FieldValue {
                column: column.name.clone(),
                value: sample_value(column, index),
            })
            .collect();
        changes.push(RowChange {
            original: None,
            kind: "insert".into(),
            pk: Vec::new(),
            values,
        });
    }
    Ok(commit_changes(
        state,
        CommitRequest {
            connection_id: request.connection_id.clone(),
            database: request.database.clone(),
            schema: request.schema.clone(),
            table: request.table.clone(),
            changes,
        },
    )?
    .applied)
}

pub fn mask_columns(state: &AppState, request: &MaskRequest) -> AppResult<String> {
    if request.columns.is_empty() {
        return Err(AppError::msg("选择要脱敏的列"));
    }
    let engine = engine_of(state, &request.connection_id)?;
    if !engine.is_sql() {
        return Err(AppError::msg(
            "文档库请在网格里改值。SQL 脱敏目前覆盖关系库。",
        ));
    }
    let name = qualified_name(engine, &request.database, &request.schema, &request.table)?;
    let sets = request
        .columns
        .iter()
        .map(|column| {
            let quoted = quote_ident(engine, column)?;
            let mask = mask_literal(column);
            Ok(format!("{quoted} = '{mask}'"))
        })
        .collect::<AppResult<Vec<_>>>()?
        .join(", ");
    let sql = format!("UPDATE {name} SET {sets}");
    let response = execute_sql(
        state,
        ExecuteSqlRequest { session_id: None,
            connection_id: request.connection_id.clone(),
            database: request.database.clone(),
            sql,
            page: 1,
            page_size: 5,
            only_statement: None,
        },
    )?;
    let first = response
        .statements
        .into_iter()
        .next()
        .ok_or_else(|| AppError::msg("没有结果"))?;
    if !first.ok {
        return Err(AppError::msg(
            first.error.unwrap_or_else(|| "脱敏失败".into()),
        ));
    }
    Ok(format!("已更新 {} 行", first.rows_affected))
}

pub(super) fn remap_cell(engine: Engine, value: &JsonValue) -> JsonValue {
    let _ = catalogdiff::map_cell_for_engine(engine, value);
    value.clone()
}

#[cfg(test)]
pub(super) fn load_all(
    state: &AppState,
    id: &str,
    database: &str,
    schema: &str,
    table: &str,
) -> AppResult<(Vec<ColumnMeta>, Vec<Vec<JsonValue>>, u64, bool)> {
    load_all_with_ceiling(state, id, database, schema, table, CEILING)
}

pub(super) fn load_all_operation(
    state: &AppState,
    id: &str,
    database: &str,
    schema: &str,
    table: &str,
    operation: &OperationContext,
) -> AppResult<(Vec<ColumnMeta>, Vec<Vec<JsonValue>>, u64, bool)> {
    load_all_with_ceiling_operation(state, id, database, schema, table, CEILING, operation)
}

#[cfg(test)]
pub(super) fn load_all_with_ceiling(
    state: &AppState,
    id: &str,
    database: &str,
    schema: &str,
    table: &str,
    ceiling: u32,
) -> AppResult<(Vec<ColumnMeta>, Vec<Vec<JsonValue>>, u64, bool)> {
    load_all_with_ceiling_operation(
        state,
        id,
        database,
        schema,
        table,
        ceiling,
        &OperationContext::detached(),
    )
}

pub(super) fn load_all_with_ceiling_operation(
    state: &AppState,
    id: &str,
    database: &str,
    schema: &str,
    table: &str,
    ceiling: u32,
    operation: &OperationContext,
) -> AppResult<(Vec<ColumnMeta>, Vec<Vec<JsonValue>>, u64, bool)> {
    let mut read = ReadSession::open(state, id, database, operation)?;
    let mut reader = TableReader::new(
        &mut *read.session,
        read.snapshot,
        database,
        schema,
        table,
        Some(ceiling),
        operation,
    )?;
    let mut rows = Vec::new();
    while let Some(batch) = reader.next(operation)? {
        rows.extend(batch);
    }
    let result = (
        reader.columns.clone(),
        rows,
        reader.total,
        reader.truncated(),
    );
    drop(reader);
    read.finish()?;
    operation.checkpoint()?;
    Ok(result)
}

pub(super) fn ensure_complete(
    total: u64,
    rows: usize,
    truncated: bool,
    operation: &str,
) -> AppResult<()> {
    if truncated || total != rows as u64 {
        return Err(AppError::msg(format!("{operation}已取消：未完整读取全部数据（共 {total} 行，读取 {rows} 行；内存上限 {CEILING} 行）。没有开始写入目标或生成备份，请缩小数据范围后重试")));
    }
    Ok(())
}

pub(super) fn columns_of(
    state: &AppState,
    id: &str,
    database: &str,
    schema: &str,
    table: &str,
) -> AppResult<Vec<ColumnMeta>> {
    state.with_session(id, |session| session.list_columns(database, schema, table))
}

fn sample_value(column: &ColumnMeta, index: u32) -> JsonValue {
    let kind = column.data_type.to_ascii_lowercase();
    let name = column.name.to_ascii_lowercase();
    if name.contains("mail") {
        return JsonValue::String(format!("user{index}@example.com"));
    }
    if name.contains("name") || name.contains("title") {
        return JsonValue::String(format!("样本{index}"));
    }
    if kind.contains("int") || kind.contains("serial") {
        return json!(index);
    }
    if kind.contains("real")
        || kind.contains("float")
        || kind.contains("double")
        || kind.contains("numeric")
        || kind.contains("decimal")
    {
        return json!(index as f64 + 0.5);
    }
    if kind.contains("bool") {
        return JsonValue::Bool(index % 2 == 0);
    }
    if kind.contains("date") || kind.contains("time") {
        return JsonValue::String(format!("2024-01-{:02}", (index % 28) + 1));
    }
    JsonValue::String(format!("样本{index}"))
}

fn mask_literal(column: &str) -> String {
    let name = column.to_ascii_lowercase();
    if name.contains("mail") {
        "hidden@example.com".into()
    } else if name.contains("phone") || name.contains("mobile") {
        "00000000000".into()
    } else if name.contains("name") {
        "某用户".into()
    } else {
        "***".into()
    }
}
