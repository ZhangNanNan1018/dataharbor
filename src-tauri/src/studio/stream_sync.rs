//! Full-table synchronization stages complete snapshots on disk before writing.
use super::{
    compatible_keys, key_family, pk_values, reject_same_object, transfer_legacy, zip_values,
};
use crate::data::{commit_changes_on_session, validate_changes};
use crate::drivers::DbSession;
use crate::error::{AppError, AppResult};
use crate::models::{ColumnMeta, CommitRequest, Engine, RowChange};
use crate::operations::OperationContext;
use crate::state::AppState;
use crate::studio::storage::engine_of;
use crate::studio::table_data::{remap_cell, BATCH, CEILING};
use crate::studio::types::{PairRequest, SyncResult};
use serde_json::Value;

#[path = "scan_stage.rs"]
mod stage;
use stage::{canonical_value, Relation, Side, Stage};

pub(super) fn supports_pair(state: &AppState, request: &PairRequest) -> AppResult<bool> {
    let supported = |engine| {
        matches!(
            engine,
            Engine::Sqlite | Engine::Mysql | Engine::Mariadb | Engine::Postgres
        )
    };
    Ok(supported(engine_of(state, &request.source_id)?)
        && supported(engine_of(state, &request.target_id)?))
}

pub(super) fn run(
    state: &AppState,
    request: &PairRequest,
    synchronize: bool,
    operation: &OperationContext,
) -> AppResult<SyncResult> {
    run_with_hook(state, request, synchronize, operation, &mut |_, _| {})
}

fn columns(
    session: &mut dyn DbSession,
    database: &str,
    schema: &str,
    table: &str,
    operation: &OperationContext,
    target: bool,
) -> AppResult<Vec<ColumnMeta>> {
    operation.checkpoint()?;
    let interrupt = operation.bind_interrupt(session.cancel_handle(database)?)?;
    let result = (|| {
        operation.checkpoint()?;
        let columns = session.list_columns(database, schema, table)?;
        operation.checkpoint()?;
        if columns.is_empty() {
            return Err(AppError::msg("找不到表结构；没有开始写入目标"));
        }
        if target && session.relation_kind(database, schema, table)?.as_deref() != Some("table") {
            return Err(AppError::msg(
                "同步目标必须是可写的真实表；没有开始写入目标",
            ));
        }
        Ok(columns)
    })();
    drop(interrupt);
    operation.checkpoint()?;
    result
}

fn run_with_hook(
    state: &AppState,
    request: &PairRequest,
    synchronize: bool,
    operation: &OperationContext,
    hook: &mut dyn FnMut(&str, u64),
) -> AppResult<SyncResult> {
    operation.checkpoint()?;
    reject_same_object(state, request)?;
    let mut source_session = state.open_isolated_session(&request.source_id)?;
    let mut target_session = state.open_isolated_session(&request.target_id)?;
    let source_columns = columns(
        &mut *source_session,
        &request.source_database,
        &request.source_schema,
        &request.source_table,
        operation,
        false,
    )?;
    let target_columns = columns(
        &mut *target_session,
        &request.target_database,
        &request.target_schema,
        &request.target_table,
        operation,
        true,
    )?;
    let keys = if synchronize {
        compatible_keys(&source_columns, &target_columns)?
    } else {
        if source_columns.iter().any(|column| {
            !target_columns
                .iter()
                .any(|target| target.name == column.name)
        }) {
            return Err(AppError::msg("目标表缺少源表列；没有开始写入目标"));
        }
        let keys = source_columns
            .iter()
            .filter(|column| column.primary_key)
            .map(|column| column.name.clone())
            .collect::<Vec<_>>();
        if keys.is_empty() {
            drop(source_session);
            drop(target_session);
            operation.report(
                "读取源表",
                0,
                None,
                "无主键表使用受限完整读取，不采用不稳定的分页流式读取",
            );
            let mut result = transfer_legacy(state, request, CEILING, operation)?;
            result.note = format!(
                "无主键表采用受限完整读取（最多 {CEILING} 行）。{}",
                result.note
            );
            return Ok(result);
        }
        keys
    };
    let mut staging = Stage::new(state.config_dir())?;
    let source = Relation {
        database: &request.source_database,
        schema: &request.source_schema,
        table: &request.source_table,
        columns: &source_columns,
        canonical_keys: &keys,
    };
    let source_total = staging.scan(
        Side::Source,
        &mut *source_session,
        &source,
        operation,
        "读取源表",
        &mut |count| hook("读取源表", count),
    )?;
    // Release the snapshot connection before target writes, including when both
    // objects live in the same SQLite file. The stage preserves the snapshot.
    drop(source_session);
    let target_total = if synchronize {
        let target = Relation {
            database: &request.target_database,
            schema: &request.target_schema,
            table: &request.target_table,
            columns: &target_columns,
            canonical_keys: &keys,
        };
        staging.scan(
            Side::Target,
            &mut *target_session,
            &target,
            operation,
            "读取目标表",
            &mut |count| hook("读取目标表", count),
        )?
    } else {
        0
    };
    let target_engine = target_session.engine();
    operation.checkpoint()?;
    let planned = build_plan(
        &mut staging,
        &source_columns,
        &target_columns,
        &keys,
        synchronize,
        request.delete_missing,
        target_engine,
        source_total + target_total,
        operation,
        hook,
    )?;
    let total = (planned.inserted + planned.updated + planned.deleted) as u64;
    preflight(
        &mut staging,
        target_engine,
        request,
        &target_columns,
        total,
        operation,
        hook,
    )?;
    if request.preview {
        operation.report("预览", total, Some(total), "预览完成，尚未写入目标");
        return Ok(SyncResult {
            inserted: planned.inserted,
            updated: planned.updated,
            deleted: planned.deleted,
            truncated: false,
            note: format!(
                "预览未写入目标。将插入 {} 行，更新 {} 行，删除 {} 行。确认写入前请停止相关表的其他写入。",
                planned.inserted, planned.updated, planned.deleted
            ),
        });
    }
    let mut committed = SyncResult {
        inserted: 0,
        updated: 0,
        deleted: 0,
        truncated: false,
        note: String::new(),
    };
    let mut cursor = 0;
    let mut batches = 0;
    operation.report(
        "写入目标",
        0,
        Some(total),
        "全部读取与校验已完成，即将分批写入目标",
    );
    loop {
        operation.checkpoint()?;
        let page = staging.changes_page(cursor)?;
        if page.is_empty() {
            break;
        }
        cursor = page.last().map(|(id, _)| *id).unwrap_or(cursor);
        let changes = page
            .into_iter()
            .map(|(_, change)| change)
            .collect::<Vec<_>>();
        operation.report(
            "写入目标",
            (committed.inserted + committed.updated + committed.deleted) as u64,
            Some(total),
            "正在提交当前批次；取消将在当前事务结束后停止后续批次",
        );
        operation.checkpoint()?;
        let applied = commit_changes_on_session(&mut *target_session, CommitRequest {
            connection_id: request.target_id.clone(), database: request.target_database.clone(), schema: request.target_schema.clone(), table: request.target_table.clone(), changes: changes.clone(),
        }, operation).map_err(|error| match error {
            cancelled @ AppError::Cancelled { .. } => cancelled,
            other => AppError::msg(format!("第 {} 批失败；此前已提交 {batches} 批、{} 条更改，已提交批次不会回滚，请核对目标后重新对比，勿重放整个任务：{other}", batches + 1, committed.inserted + committed.updated + committed.deleted)),
        })?.applied;
        if applied != changes.len() {
            return Err(AppError::msg("批次完成数量与计划不一致，请核对目标数据"));
        }
        for change in &changes {
            match change.kind.as_str() {
                "insert" => committed.inserted += 1,
                "update" => committed.updated += 1,
                "delete" => committed.deleted += 1,
                _ => unreachable!(),
            }
        }
        batches += 1;
        let completed = (committed.inserted + committed.updated + committed.deleted) as u64;
        operation.report(
            "写入目标",
            completed,
            Some(total),
            "当前批次已提交，已提交结果不会因后续取消或失败而回滚",
        );
        hook("写入目标", completed);
        operation.checkpoint()?;
    }
    hook("batchPeak", staging.max_batch as u64);
    committed.note = format!("源 {source_total} 行{}已通过独立一致读快照完整分批读取，使用临时磁盘索引按完整主键 {} 对比；未使用全量内存行数上限。共提交 {batches} 批，每批最多 {BATCH} 条。源与目标快照独立建立；写入期间目标原值冲突会停止任务，已提交批次不会自动回滚。", if synchronize { format!("、目标 {target_total} 行") } else { String::new() }, keys.join(", "));
    Ok(committed)
}

fn build_plan(
    staging: &mut Stage,
    source: &[ColumnMeta],
    target: &[ColumnMeta],
    keys: &[String],
    synchronize: bool,
    delete_missing: bool,
    target_engine: Engine,
    total: u64,
    operation: &OperationContext,
    hook: &mut dyn FnMut(&str, u64),
) -> AppResult<SyncResult> {
    let mut result = SyncResult {
        inserted: 0,
        updated: 0,
        deleted: 0,
        truncated: false,
        note: String::new(),
    };
    let mut completed = 0;
    let mut cursor = String::new();
    loop {
        operation.checkpoint()?;
        let page = staging.input_page(Side::Source, &cursor)?;
        if page.is_empty() {
            break;
        }
        staging.begin_plan_batch()?;
        for (key, row) in &page {
            operation.checkpoint()?;
            let existing = if synchronize {
                staging.get(Side::Target, key)?
            } else {
                None
            };
            let values: Vec<crate::models::FieldValue> = zip_values(source, row)
                .into_iter()
                .map(|mut field| {
                    field.value = remap_cell(target_engine, &field.value);
                    field
                })
                .collect();
            let change = if let Some(existing) = existing {
                if equivalent_row(source, row, target, &existing)? {
                    None
                } else {
                    let values = values
                        .into_iter()
                        .filter(|field| !keys.contains(&field.column))
                        .collect::<Vec<_>>();
                    if values.is_empty() {
                        None
                    } else {
                        result.updated += 1;
                        Some(RowChange {
                            kind: "update".into(),
                            pk: pk_values(keys, target, &existing),
                            original: Some(zip_values(target, &existing)),
                            values,
                        })
                    }
                }
            } else {
                result.inserted += 1;
                Some(RowChange {
                    kind: "insert".into(),
                    pk: vec![],
                    original: None,
                    values,
                })
            };
            if let Some(change) = change {
                staging.append_change(&change)?;
            }
            completed += 1;
        }
        staging.end_plan_batch()?;
        cursor = page.last().unwrap().0.clone();
        operation.report(
            "对比差异",
            completed,
            Some(total),
            "正在逐批比较磁盘索引并生成差异计划；尚未写入目标",
        );
        hook("对比差异", completed);
    }
    if synchronize {
        cursor.clear();
        loop {
            operation.checkpoint()?;
            let page = staging.input_page(Side::Target, &cursor)?;
            if page.is_empty() {
                break;
            }
            staging.begin_plan_batch()?;
            for (key, row) in &page {
                operation.checkpoint()?;
                if delete_missing && !staging.contains_source(key)? {
                    staging.append_change(&RowChange {
                        kind: "delete".into(),
                        pk: pk_values(keys, target, row),
                        original: Some(zip_values(target, row)),
                        values: vec![],
                    })?;
                    result.deleted += 1;
                }
                completed += 1;
            }
            staging.end_plan_batch()?;
            cursor = page.last().unwrap().0.clone();
            operation.report(
                "对比差异",
                completed,
                Some(total),
                "正在逐批比较磁盘索引并生成差异计划；尚未写入目标",
            );
            hook("对比差异", completed);
        }
    }
    Ok(result)
}

fn equivalent_row(
    source: &[ColumnMeta],
    row: &[Value],
    target: &[ColumnMeta],
    existing: &[Value],
) -> AppResult<bool> {
    for (index, column) in source.iter().enumerate() {
        let target_index = target
            .iter()
            .position(|other| other.name == column.name)
            .ok_or_else(|| AppError::msg("目标缺少源表列"))?;
        let left = &row[index];
        let right = &existing[target_index];
        let equal = if key_family(&column.data_type) == key_family(&target[target_index].data_type)
        {
            canonical_value(column, left)? == canonical_value(&target[target_index], right)?
        } else {
            left == right
        };
        if !equal {
            return Ok(false);
        }
    }
    Ok(true)
}

fn preflight(
    staging: &mut Stage,
    engine: Engine,
    request: &PairRequest,
    columns: &[ColumnMeta],
    total: u64,
    operation: &OperationContext,
    hook: &mut dyn FnMut(&str, u64),
) -> AppResult<()> {
    let mut cursor = 0;
    let mut checked = 0;
    operation.report("校验变更", 0, Some(total), "正在校验全部变更；尚未写入目标");
    loop {
        operation.checkpoint()?;
        let page = staging.changes_page(cursor)?;
        if page.is_empty() {
            break;
        }
        cursor = page.last().unwrap().0;
        let changes = page
            .into_iter()
            .map(|(_, change)| change)
            .collect::<Vec<_>>();
        validate_changes(
            engine,
            &request.target_database,
            &request.target_schema,
            &request.target_table,
            columns,
            &changes,
        )?;
        checked += changes.len() as u64;
        operation.report(
            "校验变更",
            checked,
            Some(total),
            "正在校验全部变更的字段、类型和原值；尚未写入目标",
        );
        hook("校验变更", checked);
    }
    operation.checkpoint()
}

#[cfg(test)]
#[path = "stream_sync_tests.rs"]
mod tests;
