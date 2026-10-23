use super::catalog::object_snap;
use super::storage::{cell_string, engine_of, profile_of};
use super::table_data::{
    ensure_complete, load_all_with_ceiling_operation, remap_cell, BATCH, CEILING,
};
use super::types::{DiffResult, PairRequest, SyncResult};
use crate::catalogdiff;
use crate::drivers::DbSession;
use crate::error::{AppError, AppResult};
use crate::models::{ColumnMeta, CommitRequest, Engine, ExecuteSqlRequest, FieldValue, RowChange};
use crate::operations::OperationContext;
use crate::query::execute_sql;
use crate::sqlutil::qualified_name;
use crate::state::AppState;
use serde_json::Value as JsonValue;

#[path = "type_mapping.rs"]
mod type_mapping;

#[path = "stream_sync.rs"]
mod streaming;

pub fn diff_structure(state: &AppState, request: &PairRequest) -> AppResult<DiffResult> {
    if request.apply && request.selected_ids.is_empty() {
        return Err(AppError::msg(
            "没有勾选结构变更，已拒绝执行；请先对比并勾选语句",
        ));
    }
    let source = object_snap(
        state,
        &request.source_id,
        &request.source_database,
        &request.source_schema,
        &request.source_table,
    )?;
    let target = object_snap(
        state,
        &request.target_id,
        &request.target_database,
        &request.target_schema,
        &request.target_table,
    )?;
    let target_engine = engine_of(state, &request.target_id)?;
    let source_engine = engine_of(state, &request.source_id)?;
    let mut source = source;
    if source_engine != target_engine {
        for column in &mut source.columns {
            column.data_type =
                type_mapping::map_type(source_engine, target_engine, &column.data_type)
                    .map_err(|err| AppError::msg(format!("列 {}：{err}", column.name)))?;
        }
    }
    let target_name = qualified_name(
        target_engine,
        &request.target_database,
        &request.target_schema,
        &request.target_table,
    )?;
    let (mut statements, mut notes) = catalogdiff::diff_objects(
        target_engine,
        &target_name,
        &source,
        &target,
        request.allow_destructive,
    );
    if request.apply
        && request
            .selected_ids
            .iter()
            .any(|id| !statements.iter().any(|item| &item.id == id))
    {
        return Err(AppError::msg(
            "选中的结构变更已失效，源表或目标表可能发生变化；请重新对比后再执行",
        ));
    }
    if !request.selected_ids.is_empty() {
        statements.retain(|item| request.selected_ids.iter().any(|id| id == &item.id));
        notes.push(format!("只保留勾选的 {} 条语句", statements.len()));
    }
    if request.apply
        && !request.allow_destructive
        && statements.iter().any(|item| {
            item.kind == "modify" || item.kind == "view" || item.kind.starts_with("drop-")
        })
    {
        return Err(AppError::msg(
            "所选变更会修改现有结构，可能缩窄类型或丢失数据；请先允许危险结构变更并重新确认",
        ));
    }
    let sql = statements
        .iter()
        .map(|item| item.sql.trim())
        .filter(|item| !item.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let mut applied = false;
    if request.apply && !sql.trim().is_empty() {
        let response = execute_sql(
            state,
            ExecuteSqlRequest { session_id: None,
                connection_id: request.target_id.clone(),
                database: request.target_database.clone(),
                sql: sql.clone(),
                page: 1,
                page_size: 20,
                only_statement: None,
            },
        )?;
        if let Some(failed) = response.statements.iter().find(|item| !item.ok) {
            return Err(AppError::msg(
                failed
                    .error
                    .clone()
                    .unwrap_or_else(|| "结构同步失败".into()),
            ));
        }
        applied = true;
    }
    Ok(DiffResult {
        sql,
        notes,
        applied,
        statements,
    })
}

pub fn transfer_rows_with_operation(
    state: &AppState,
    request: &PairRequest,
    operation: &OperationContext,
) -> AppResult<SyncResult> {
    reject_same_object(state, request)?;
    if streaming::supports_pair(state, request)? {
        streaming::run(state, request, false, operation)
    } else {
        operation.report(
            "reading",
            0,
            None,
            "当前引擎使用受限读取；取消会在读取阶段结束或当前批次提交后生效",
        );
        transfer_legacy(state, request, CEILING, operation)
    }
}

#[cfg(test)]
pub(super) fn transfer_with_ceiling(
    state: &AppState,
    request: &PairRequest,
    ceiling: u32,
) -> AppResult<SyncResult> {
    transfer_legacy(state, request, ceiling, &OperationContext::detached())
}

fn transfer_legacy(
    state: &AppState,
    request: &PairRequest,
    ceiling: u32,
    operation: &OperationContext,
) -> AppResult<SyncResult> {
    operation.checkpoint()?;
    reject_same_object(state, request)?;
    let source_profile = profile_of(state, &request.source_id)?;
    let target_profile = profile_of(state, &request.target_id)?;
    let mut target_session = state.open_isolated_session(&request.target_id)?;
    let (columns, rows, total, hit_ceiling) = load_all_with_ceiling_operation(
        state,
        &request.source_id,
        &request.source_database,
        &request.source_schema,
        &request.source_table,
        ceiling,
        operation,
    )?;
    ensure_complete(total, rows.len(), hit_ceiling, "数据传输")?;
    let target_engine = target_session.engine();
    let names: Vec<String> = columns.iter().map(|column| column.name.clone()).collect();
    let changes = rows
        .iter()
        .map(|row| RowChange {
            kind: "insert".into(),
            pk: Vec::new(),
            original: None,
            values: names
                .iter()
                .cloned()
                .zip(row.iter().cloned())
                .map(|(column, value)| FieldValue {
                    column,
                    value: remap_cell(target_engine, &value),
                })
                .collect(),
        })
        .collect::<Vec<_>>();
    operation.checkpoint()?;
    ensure_unchanged_profiles(state, request, &source_profile, &target_profile)?;
    if request.preview {
        return Ok(SyncResult {
            inserted: rows.len(),
            updated: 0,
            deleted: 0,
            truncated: false,
            note: format!(
                "预览未写入目标。传输将插入 {} 行，不按主键合并已有行。确认写入前请停止源表的其他写入。",
                rows.len()
            ),
        });
    }
    let inserted = commit_legacy_batches(
        &mut *target_session,
        operation,
        &request.target_id,
        &request.target_database,
        &request.target_schema,
        &request.target_table,
        changes,
    )?;
    Ok(SyncResult {
        inserted,
        updated: 0,
        deleted: 0,
        truncated: false,
        note: format!("已完整读取源数据，按 {BATCH} 行一批插入。每批独立提交，失败前已提交的批次不会自动撤销。读取期间请停止源表写入。"),
    })
}

#[cfg(test)]
pub fn sync_rows(state: &AppState, request: &PairRequest) -> AppResult<SyncResult> {
    sync_rows_with_operation(state, request, &OperationContext::detached())
}

pub fn sync_rows_with_operation(
    state: &AppState,
    request: &PairRequest,
    operation: &OperationContext,
) -> AppResult<SyncResult> {
    reject_same_object(state, request)?;
    if streaming::supports_pair(state, request)? {
        streaming::run(state, request, true, operation)
    } else {
        operation.report(
            "reading",
            0,
            None,
            "当前引擎使用受限读取；取消会在读取阶段结束或当前批次提交后生效",
        );
        sync_legacy(state, request, CEILING, operation)
    }
}

#[cfg(test)]
pub(super) fn sync_with_ceiling(
    state: &AppState,
    request: &PairRequest,
    ceiling: u32,
) -> AppResult<SyncResult> {
    sync_legacy(state, request, ceiling, &OperationContext::detached())
}

fn sync_legacy(
    state: &AppState,
    request: &PairRequest,
    ceiling: u32,
    operation: &OperationContext,
) -> AppResult<SyncResult> {
    operation.checkpoint()?;
    reject_same_object(state, request)?;
    let source_profile = profile_of(state, &request.source_id)?;
    let target_profile = profile_of(state, &request.target_id)?;
    let mut target_session = state.open_isolated_session(&request.target_id)?;
    let (source_columns, source_rows, source_total, source_cap) = load_all_with_ceiling_operation(
        state,
        &request.source_id,
        &request.source_database,
        &request.source_schema,
        &request.source_table,
        ceiling,
        operation,
    )?;
    operation.checkpoint()?;
    let (target_columns, target_rows, target_total, target_cap) = load_all_with_ceiling_operation(
        state,
        &request.target_id,
        &request.target_database,
        &request.target_schema,
        &request.target_table,
        ceiling,
        operation,
    )?;
    ensure_complete(
        source_total,
        source_rows.len(),
        source_cap,
        "数据同步（源表）",
    )?;
    ensure_complete(
        target_total,
        target_rows.len(),
        target_cap,
        "数据同步（目标表）",
    )?;
    let keys = compatible_keys(&source_columns, &target_columns)?;
    let source_index = index_rows(&source_columns, &source_rows, &keys)?;
    let target_index = index_rows(&target_columns, &target_rows, &keys)?;
    let mut changes = Vec::new();
    for (key, row) in &source_index {
        let values = zip_values(&source_columns, row);
        if let Some(existing) = target_index.get(key) {
            let current = source_columns
                .iter()
                .map(|column| {
                    let index = target_columns
                        .iter()
                        .position(|item| item.name == column.name)
                        .ok_or_else(|| AppError::msg(format!("目标表缺少列 {}", column.name)))?;
                    Ok(FieldValue {
                        column: column.name.clone(),
                        value: existing[index].clone(),
                    })
                })
                .collect::<AppResult<Vec<_>>>()?;
            if values != current {
                changes.push(RowChange {
                    kind: "update".into(),
                    pk: pk_values(&keys, &source_columns, row),
                    original: Some(zip_values(&target_columns, existing)),
                    values: values
                        .into_iter()
                        .filter(|item| !keys.iter().any(|key| key == &item.column))
                        .collect(),
                });
            }
        } else {
            changes.push(RowChange {
                kind: "insert".into(),
                pk: Vec::new(),
                original: None,
                values,
            });
        }
    }
    let mut deleted = 0;
    if request.delete_missing {
        for (key, row) in &target_index {
            if !source_index.contains_key(key) {
                changes.push(RowChange {
                    kind: "delete".into(),
                    pk: pk_values(&keys, &target_columns, row),
                    original: Some(zip_values(&target_columns, row)),
                    values: Vec::new(),
                });
                deleted += 1;
            }
        }
    }
    let inserted = changes
        .iter()
        .filter(|change| change.kind == "insert")
        .count();
    let updated = changes
        .iter()
        .filter(|change| change.kind == "update")
        .count();
    let target_engine = target_session.engine();
    for change in &mut changes {
        for value in &mut change.values {
            value.value = remap_cell(target_engine, &value.value);
        }
    }
    ensure_unchanged_profiles(state, request, &source_profile, &target_profile)?;
    if request.preview {
        return Ok(SyncResult {
            inserted,
            updated,
            deleted,
            truncated: false,
            note: format!(
                "预览未写入目标。将插入 {inserted} 行，更新 {updated} 行，删除 {deleted} 行。主键：{}。确认写入前请停止源表和目标表的其他写入。",
                keys.join(", ")
            ),
        });
    }
    if !changes.is_empty() {
        commit_legacy_batches(
            &mut *target_session,
            operation,
            &request.target_id,
            &request.target_database,
            &request.target_schema,
            &request.target_table,
            changes,
        )?;
    }
    Ok(SyncResult {
        inserted,
        updated,
        deleted,
        truncated: false,
        note: format!(
            "已完整读取两张表，按真实主键 {} 对比。复合键使用 JSON 数组编码。每批 {BATCH} 行独立提交，失败前已提交的批次不会自动撤销。读取期间请停止两表写入。",
            keys.join(", ")
        ),
    })
}

fn commit_legacy_batches(
    session: &mut dyn DbSession,
    operation: &OperationContext,
    connection_id: &str,
    database: &str,
    schema: &str,
    table: &str,
    changes: Vec<RowChange>,
) -> AppResult<usize> {
    let mut applied = 0;
    for (index, chunk) in changes.chunks(BATCH as usize).enumerate() {
        operation.checkpoint()?;
        let count = crate::data::commit_changes_on_session(session, CommitRequest {
            connection_id: connection_id.into(), database: database.into(), schema: schema.into(),
            table: table.into(), changes: chunk.to_vec(),
        }, operation).map_err(|err| match err {
            cancelled @ AppError::Cancelled { .. } => cancelled,
            other => AppError::msg(format!("第 {} 批失败；此前已提交 {} 批、{} 条更改，已提交批次不会回滚，勿重放整个任务：{other}", index + 1, index, applied)),
        })?.applied;
        applied += count;
        operation.report(
            "writing",
            applied as u64,
            Some(changes.len() as u64),
            "正在分批写入，已提交批次不会自动回滚",
        );
        operation.checkpoint()?;
    }
    Ok(applied)
}

fn ensure_unchanged_profiles(
    state: &AppState,
    request: &PairRequest,
    source: &crate::models::StoredProfile,
    target: &crate::models::StoredProfile,
) -> AppResult<()> {
    let current_source = profile_of(state, &request.source_id)?;
    let current_target = profile_of(state, &request.target_id)?;
    let value = |profile: &crate::models::StoredProfile| {
        serde_json::to_value(profile).map_err(|err| AppError::msg(err.to_string()))
    };
    if value(source)? != value(&current_source)? || value(target)? != value(&current_target)? {
        return Err(AppError::msg(
            "读取期间连接配置发生变化，已取消；没有开始写入目标",
        ));
    }
    Ok(())
}

fn reject_same_object(state: &AppState, request: &PairRequest) -> AppResult<()> {
    let source_profile = profile_of(state, &request.source_id)?;
    let target_profile = profile_of(state, &request.target_id)?;
    let engine = source_profile.engine;
    if engine != target_profile.engine {
        return Ok(());
    }
    let same_connection = if engine == Engine::Sqlite {
        request.source_id == request.target_id
            || std::fs::canonicalize(&source_profile.file_path)
                .ok()
                .zip(std::fs::canonicalize(&target_profile.file_path).ok())
                .is_some_and(|(source, target)| source == target)
    } else {
        request.source_id == request.target_id
            || (source_profile
                .host
                .eq_ignore_ascii_case(&target_profile.host)
                && source_profile.port == target_profile.port
                && source_profile.ssh_enabled == target_profile.ssh_enabled
                && (!source_profile.ssh_enabled
                    || (source_profile.ssh_host == target_profile.ssh_host
                        && source_profile.ssh_port == target_profile.ssh_port))
                && source_profile.http_tunnel == target_profile.http_tunnel
                && (!source_profile.http_tunnel
                    || (source_profile.http_proxy_host == target_profile.http_proxy_host
                        && source_profile.http_proxy_port == target_profile.http_proxy_port)))
    };
    let source_db = if request.source_database.is_empty() {
        &source_profile.database
    } else {
        &request.source_database
    };
    let target_db = if request.target_database.is_empty() {
        &target_profile.database
    } else {
        &request.target_database
    };
    let same_database = engine == Engine::Sqlite || source_db == target_db;
    let source = qualified_name(
        engine,
        source_db,
        &request.source_schema,
        &request.source_table,
    )?;
    let target = qualified_name(
        engine,
        target_db,
        &request.target_schema,
        &request.target_table,
    )?;
    if same_connection && same_database && source == target {
        return Err(AppError::msg(
            "源和目标是同一对象，已拒绝传输或同步，避免复制或覆盖自身数据",
        ));
    }
    Ok(())
}

fn compatible_keys(source: &[ColumnMeta], target: &[ColumnMeta]) -> AppResult<Vec<String>> {
    let keys = source
        .iter()
        .filter(|column| column.primary_key)
        .map(|column| column.name.clone())
        .collect::<Vec<_>>();
    let target_keys = target
        .iter()
        .filter(|column| column.primary_key)
        .map(|column| column.name.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    if keys.is_empty() || target_keys.is_empty() {
        return Err(AppError::msg(
            "按主键同步要求源表和目标表都有真实主键，不会改用第一列",
        ));
    }
    if keys
        .iter()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>()
        != target_keys
    {
        return Err(AppError::msg("源表和目标表的主键列不一致，已取消同步"));
    }
    for name in &keys {
        let source_column = source.iter().find(|column| column.name == *name).unwrap();
        let target_column = target.iter().find(|column| column.name == *name).unwrap();
        if key_family(&source_column.data_type) != key_family(&target_column.data_type) {
            return Err(AppError::msg(format!(
                "主键列 {name} 的类型不兼容，已取消同步"
            )));
        }
    }
    if source
        .iter()
        .any(|column| !target.iter().any(|other| other.name == column.name))
    {
        return Err(AppError::msg("目标表缺少源表列，请先完成结构对齐"));
    }
    Ok(keys)
}

fn key_family(data_type: &str) -> &'static str {
    let kind = data_type.to_ascii_lowercase();
    if crate::sqlutil::is_numeric_type(&kind) {
        "number"
    } else if kind.contains("bool") || kind == "bit" {
        "bool"
    } else if ["blob", "binary", "bytea"]
        .iter()
        .any(|token| kind.contains(token))
    {
        "binary"
    } else {
        "text"
    }
}

fn index_rows<'a>(
    columns: &[ColumnMeta],
    rows: &'a [Vec<JsonValue>],
    keys: &[String],
) -> AppResult<std::collections::BTreeMap<String, &'a Vec<JsonValue>>> {
    let mut map = std::collections::BTreeMap::new();
    for row in rows {
        let values = keys
            .iter()
            .map(|name| {
                let index = columns
                    .iter()
                    .position(|column| column.name == *name)
                    .ok_or_else(|| AppError::msg(format!("缺少主键列 {name}")))?;
                let value = row
                    .get(index)
                    .filter(|value| !value.is_null())
                    .ok_or_else(|| {
                        AppError::msg(format!("主键列 {name} 存在空值或缺失，已取消同步"))
                    })?;
                // PostgreSQL's text protocol returns scalar numbers as strings.
                // Normalize only numeric keys, while retaining document value types.
                Ok(if key_family(&columns[index].data_type) == "number" {
                    JsonValue::String(cell_string(Some(value)))
                } else {
                    value.clone()
                })
            })
            .collect::<AppResult<Vec<_>>>()?;
        let key = serde_json::to_string(&values).map_err(|err| AppError::msg(err.to_string()))?;
        if map.insert(key, row).is_some() {
            return Err(AppError::msg(
                "检测到重复主键，已取消同步；没有开始写入目标",
            ));
        }
    }
    Ok(map)
}

fn zip_values(columns: &[ColumnMeta], row: &[JsonValue]) -> Vec<FieldValue> {
    columns
        .iter()
        .enumerate()
        .map(|(index, column)| FieldValue {
            column: column.name.clone(),
            value: row.get(index).cloned().unwrap_or(JsonValue::Null),
        })
        .collect()
}

fn pk_values(keys: &[String], columns: &[ColumnMeta], row: &[JsonValue]) -> Vec<FieldValue> {
    keys.iter()
        .filter_map(|name| {
            let index = columns.iter().position(|column| column.name == *name)?;
            Some(FieldValue {
                column: name.clone(),
                value: row.get(index).cloned().unwrap_or(JsonValue::Null),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::index_rows;
    use crate::models::ColumnMeta;
    use serde_json::json;

    #[test]
    fn duplicate_primary_keys_are_rejected() {
        let mut key = ColumnMeta::named("id");
        key.primary_key = true;
        key.data_type = "INTEGER".into();
        let rows = vec![vec![json!(1)], vec![json!(1)]];
        assert!(index_rows(&[key], &rows, &["id".into()]).is_err());
    }
}
