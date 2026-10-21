use super::storage::engine_of;
use super::types::{GenerateRequest, MaskRequest};
use crate::catalogdiff;
use crate::data::commit_changes;
use crate::error::{AppError, AppResult};
use crate::models::{ColumnMeta, CommitRequest, Engine, ExecuteSqlRequest, FieldValue, RowChange};
use crate::query::execute_sql;
use crate::sqlutil::{paginate_sql, qualified_name, quote_ident};
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
        ExecuteSqlRequest {
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

pub(super) fn commit_batches(
    state: &AppState,
    connection_id: &str,
    database: &str,
    schema: &str,
    table: &str,
    changes: Vec<RowChange>,
) -> AppResult<usize> {
    if changes.is_empty() {
        return Ok(0);
    }
    let mut applied = 0;
    for (index, chunk) in changes.chunks(BATCH as usize).enumerate() {
        applied += commit_changes(
            state,
            CommitRequest {
                connection_id: connection_id.to_string(),
                database: database.to_string(),
                schema: schema.to_string(),
                table: table.to_string(),
                changes: chunk.to_vec(),
            },
        ).map_err(|err| AppError::msg(format!("第 {} 批失败；此前已提交 {} 批、{} 条更改，已提交批次不会回滚，请核对目标数据后重新对比，勿直接重放整个任务：{err}", index + 1, index, applied)))?
        .applied;
    }
    Ok(applied)
}

pub(super) fn load_all(
    state: &AppState,
    id: &str,
    database: &str,
    schema: &str,
    table: &str,
) -> AppResult<(Vec<ColumnMeta>, Vec<Vec<JsonValue>>, u64, bool)> {
    load_all_with_ceiling(state, id, database, schema, table, CEILING)
}

pub(super) fn load_all_with_ceiling(
    state: &AppState,
    id: &str,
    database: &str,
    schema: &str,
    table: &str,
    ceiling: u32,
) -> AppResult<(Vec<ColumnMeta>, Vec<Vec<JsonValue>>, u64, bool)> {
    let mut offset = 0u32;
    let mut columns = Vec::new();
    let mut rows = Vec::new();
    let mut total = None;
    let mut hit = false;
    loop {
        let limit = BATCH.min(ceiling.saturating_sub(offset));
        if limit == 0 {
            break;
        }
        let (page_columns, page_rows, page_total, page_truncated) =
            load_range(state, id, database, schema, table, limit, offset)?;
        if total.is_some_and(|expected| expected != page_total) {
            return Err(AppError::msg(
                "读取期间表的行数发生变化，请停止写入源表后重试",
            ));
        }
        total = Some(page_total);
        if columns.is_empty() {
            columns = page_columns;
            // Without a unique ordering key, read once instead of using OFFSET pages.
            if !columns.iter().any(|column| column.primary_key) && page_total > u64::from(limit) {
                let (all_columns, all_rows, final_total, truncated) =
                    load_range(state, id, database, schema, table, ceiling, 0)?;
                if final_total != page_total {
                    return Err(AppError::msg(
                        "读取期间表的行数发生变化，请停止写入源表后重试",
                    ));
                }
                let incomplete = truncated || final_total > all_rows.len() as u64;
                return Ok((all_columns, all_rows, final_total, incomplete));
            }
        } else if columns
            .iter()
            .map(|column| &column.name)
            .ne(page_columns.iter().map(|column| &column.name))
        {
            return Err(AppError::msg("读取期间表结构发生变化，请重试"));
        }
        let count = page_rows.len() as u32;
        rows.extend(page_rows);
        offset += count;
        if page_truncated {
            hit = true;
            break;
        }
        if count < limit || u64::from(offset) >= page_total {
            break;
        }
    }
    let total = total.unwrap_or(0);
    hit |= total > rows.len() as u64;
    Ok((columns, rows, total, hit))
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

pub(super) fn load_range(
    state: &AppState,
    id: &str,
    database: &str,
    schema: &str,
    table: &str,
    limit: u32,
    offset: u32,
) -> AppResult<(Vec<ColumnMeta>, Vec<Vec<JsonValue>>, u64, bool)> {
    state.with_session(id, |session| {
        if session.is_document_store() {
            let total = session.count_relation(database, schema, table)?;
            let page = session.browse_page(database, schema, table, &[], &[], limit, offset)?;
            let columns = if page.columns.is_empty() {
                session.list_columns(database, schema, table)?
            } else {
                page.columns
            };
            return Ok((columns, page.rows, total, page.truncated));
        }
        let columns = session.list_columns(database, schema, table)?;
        if columns.is_empty() {
            return Err(AppError::msg("找不到表结构，已停止读取"));
        }
        let name = qualified_name(session.engine(), database, schema, table)?;
        let select_columns = columns
            .iter()
            .map(|column| quote_ident(session.engine(), &column.name))
            .collect::<AppResult<Vec<_>>>()?
            .join(", ");
        let keys = columns
            .iter()
            .filter(|column| column.primary_key)
            .map(|column| quote_ident(session.engine(), &column.name))
            .collect::<AppResult<Vec<_>>>()?;
        let order = if keys.is_empty() {
            String::new()
        } else {
            format!(" ORDER BY {}", keys.join(", "))
        };
        let sql = paginate_sql(
            session.engine(),
            &format!("SELECT {select_columns} FROM {name}{order}"),
            limit,
            offset,
        );
        let count = session.execute(database, &format!("SELECT COUNT(*) FROM {name}"), Some(1))?;
        let total = count
            .rows
            .first()
            .and_then(|row| row.first())
            .and_then(crate::sqlutil::json_count)
            .ok_or_else(|| AppError::msg("无法确认表的完整行数，已停止读取"))?;
        let outcome = session.execute(database, &sql, Some(limit as usize))?;
        let ordered_columns = if outcome.columns.is_empty() {
            columns
        } else {
            outcome
                .columns
                .iter()
                .map(|result| {
                    columns
                        .iter()
                        .find(|column| column.name == result.name)
                        .or_else(|| {
                            columns
                                .iter()
                                .find(|column| column.name.eq_ignore_ascii_case(&result.name))
                        })
                        .cloned()
                        .ok_or_else(|| {
                            AppError::msg(format!("查询结果列 {} 不属于表结构", result.name))
                        })
                })
                .collect::<AppResult<Vec<_>>>()?
        };
        if outcome
            .rows
            .iter()
            .any(|row| row.len() != ordered_columns.len())
        {
            return Err(AppError::msg("查询结果列数与表结构不一致"));
        }
        Ok((ordered_columns, outcome.rows, total, outcome.truncated))
    })
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
