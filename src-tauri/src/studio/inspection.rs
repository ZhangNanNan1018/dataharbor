use super::query_tools::note_grid;
use super::storage::engine_of;
use super::table_data::columns_of;
use super::types::{GridResult, ObjectRef};
use crate::error::AppResult;
use crate::sqlutil::{qualified_name, quote_ident};
use crate::state::AppState;
use serde_json::{json, Value as JsonValue};

pub fn analyze_schema(state: &AppState, request: &ObjectRef) -> AppResult<GridResult> {
    let tables = state.with_session(&request.connection_id, |session| {
        session.list_tables(&request.database, &request.schema)
    })?;
    let engine = engine_of(state, &request.connection_id)?;
    let mut rows = Vec::new();
    for table in tables.iter().take(40) {
        let columns = columns_of(
            state,
            &request.connection_id,
            &request.database,
            &request.schema,
            table,
        )
        .unwrap_or_default();
        let pk = columns.iter().any(|column| column.primary_key);
        let nullable = columns.iter().filter(|column| column.nullable).count();
        let count = if engine.is_sql() {
            let name = qualified_name(engine, &request.database, &request.schema, table)
                .unwrap_or_else(|_| table.clone());
            state
                .with_session(&request.connection_id, |session| {
                    session.execute(
                        &request.database,
                        &format!("SELECT COUNT(*) FROM {name}"),
                        Some(1),
                    )
                })
                .ok()
                .and_then(|outcome| outcome.rows.first().and_then(|row| row.first()).cloned())
                .unwrap_or(JsonValue::Null)
        } else {
            state
                .with_session(&request.connection_id, |session| {
                    session.count_relation(&request.database, &request.schema, table)
                })
                .map(|count| json!(count))
                .unwrap_or(JsonValue::Null)
        };
        rows.push(vec![
            JsonValue::String(table.clone()),
            count,
            json!(columns.len()),
            JsonValue::Bool(pk),
            json!(nullable),
        ]);
    }
    Ok(GridResult {
        columns: vec![
            "对象".into(),
            "行数".into(),
            "列数".into(),
            "有主键".into(),
            "可空列".into(),
        ],
        rows,
        note: "最多分析 40 个对象。".into(),
    })
}

pub fn profile_table(state: &AppState, request: &ObjectRef) -> AppResult<GridResult> {
    let columns = columns_of(
        state,
        &request.connection_id,
        &request.database,
        &request.schema,
        &request.table,
    )?;
    let engine = engine_of(state, &request.connection_id)?;
    if !engine.is_sql() {
        return Ok(note_grid("文档库请用结构分析。画像查询只覆盖 SQL 表。"));
    }
    let name = qualified_name(engine, &request.database, &request.schema, &request.table)?;
    let mut rows = Vec::new();
    for column in columns.iter().take(24) {
        let quoted = quote_ident(engine, &column.name)?;
        let sql = format!(
            "SELECT COUNT(*) AS n, COUNT(DISTINCT {quoted}) AS distinct_n, MIN({quoted}) AS min_v, MAX({quoted}) AS max_v, SUM(CASE WHEN {quoted} IS NULL THEN 1 ELSE 0 END) AS nulls FROM {name}"
        );
        match state.with_session(&request.connection_id, |session| {
            session.execute(&request.database, &sql, Some(1))
        }) {
            Ok(outcome) => {
                let row = outcome.rows.first().cloned().unwrap_or_default();
                rows.push(vec![
                    JsonValue::String(column.name.clone()),
                    JsonValue::String(column.data_type.clone()),
                    row.first().cloned().unwrap_or(JsonValue::Null),
                    row.get(1).cloned().unwrap_or(JsonValue::Null),
                    row.get(2).cloned().unwrap_or(JsonValue::Null),
                    row.get(3).cloned().unwrap_or(JsonValue::Null),
                    row.get(4).cloned().unwrap_or(JsonValue::Null),
                ]);
            }
            Err(err) => rows.push(vec![
                JsonValue::String(column.name.clone()),
                JsonValue::String(column.data_type.clone()),
                JsonValue::String(err.to_string()),
                JsonValue::Null,
                JsonValue::Null,
                JsonValue::Null,
                JsonValue::Null,
            ]),
        }
    }
    Ok(GridResult {
        columns: vec![
            "列".into(),
            "类型".into(),
            "行数".into(),
            "去重".into(),
            "最小".into(),
            "最大".into(),
            "空值".into(),
        ],
        rows,
        note: "最多画像 24 列。MIN/MAX 对大文本列可能较慢。".into(),
    })
}
