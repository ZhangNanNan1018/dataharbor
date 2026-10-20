use super::storage::{cell_string, engine_of};
use super::table_data::columns_of;
use super::types::{ErEdge, ErGraph, ErTable, ForeignKeyRef, GridResult, ObjectRef};
use crate::catalogdiff::{FkSnap, IndexSnap, ObjectSnap};
use crate::error::{AppError, AppResult};
use crate::models::Engine;
use crate::sqlutil::{quote_ident, sql_literal};
use crate::state::AppState;
use serde_json::Value as JsonValue;

pub fn er_diagram(state: &AppState, request: &ObjectRef) -> AppResult<ErGraph> {
    state.with_session(&request.connection_id, |session| {
        let tables = session.list_tables(&request.database, &request.schema)?;
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        let mut note = String::new();
        for table in tables.iter().take(80) {
            let columns = session
                .list_columns(&request.database, &request.schema, table)
                .unwrap_or_default();
            if session.engine() == Engine::Sqlite {
                let quoted = quote_ident(Engine::Sqlite, table).unwrap_or_else(|_| "\"\"".into());
                if let Ok(outcome) = session.execute(
                    &request.database,
                    &format!("PRAGMA foreign_key_list({quoted})"),
                    None,
                ) {
                    for row in outcome.rows {
                        let to_table = cell_string(row.get(2));
                        let from_column = cell_string(row.get(3));
                        let to_column = cell_string(row.get(4));
                        if !to_table.is_empty() && !from_column.is_empty() {
                            edges.push(ErEdge {
                                from_table: table.clone(),
                                from_column,
                                to_table,
                                to_column,
                            });
                        }
                    }
                }
            }
            nodes.push(ErTable {
                name: table.clone(),
                columns,
            });
        }
        if session.engine() != Engine::Sqlite && session.engine().is_sql() {
            if let Some(sql) = fk_query(session.engine(), &request.database, &request.schema) {
                match session.execute(&request.database, &sql, Some(500)) {
                    Ok(outcome) => {
                        for row in outcome.rows {
                            edges.push(ErEdge {
                                from_table: cell_string(row.first()),
                                from_column: cell_string(row.get(1)),
                                to_table: cell_string(row.get(2)),
                                to_column: cell_string(row.get(3)),
                            });
                        }
                    }
                    Err(err) => note = format!("外键查询失败，仍显示表结构：{err}"),
                }
            } else {
                note = "这个引擎暂不读取外键，图中只有表和列。".into();
            }
        }
        if !session.engine().is_sql() {
            note = "文档库没有外键。这里列出集合或键，方便对照结构。".into();
        }
        Ok(ErGraph {
            tables: nodes,
            edges: edges
                .into_iter()
                .filter(|edge| !edge.from_table.is_empty() && !edge.to_table.is_empty())
                .collect(),
            note,
        })
    })
}

fn fk_query(engine: Engine, database: &str, schema: &str) -> Option<String> {
    match engine {
        Engine::Mysql | Engine::Mariadb => {
            let schema = if database.is_empty() { schema } else { database };
            let literal = sql_literal(engine, &JsonValue::String(schema.to_string())).ok()?;
            Some(format!(
                "SELECT TABLE_NAME, COLUMN_NAME, REFERENCED_TABLE_NAME, REFERENCED_COLUMN_NAME \
                 FROM information_schema.KEY_COLUMN_USAGE \
                 WHERE TABLE_SCHEMA = {literal} AND REFERENCED_TABLE_NAME IS NOT NULL"
            ))
        }
        Engine::Postgres => {
            let schema = if schema.is_empty() { "public" } else { schema };
            let literal = sql_literal(engine, &JsonValue::String(schema.to_string())).ok()?;
            Some(format!(
                "SELECT tc.table_name, kcu.column_name, ccu.table_name, ccu.column_name \
                 FROM information_schema.table_constraints tc \
                 JOIN information_schema.key_column_usage kcu \
                   ON tc.constraint_name = kcu.constraint_name AND tc.table_schema = kcu.table_schema \
                 JOIN information_schema.constraint_column_usage ccu \
                   ON ccu.constraint_name = tc.constraint_name AND ccu.table_schema = tc.table_schema \
                 WHERE tc.constraint_type = 'FOREIGN KEY' AND tc.table_schema = {literal}"
            ))
        }
        Engine::Mssql => Some(
            "SELECT OBJECT_NAME(fk.parent_object_id), COL_NAME(fkc.parent_object_id, fkc.parent_column_id), \
             OBJECT_NAME(fk.referenced_object_id), COL_NAME(fkc.referenced_object_id, fkc.referenced_column_id) \
             FROM sys.foreign_keys fk \
             JOIN sys.foreign_key_columns fkc ON fk.object_id = fkc.constraint_object_id"
                .into(),
        ),
        Engine::Oracle | Engine::Dameng => Some(
            "SELECT a.table_name, a.column_name, c_pk.table_name, b.column_name \
             FROM all_cons_columns a \
             JOIN all_constraints c ON a.owner = c.owner AND a.constraint_name = c.constraint_name \
             JOIN all_constraints c_pk ON c.r_owner = c_pk.owner AND c.r_constraint_name = c_pk.constraint_name \
             JOIN all_cons_columns b ON c_pk.owner = b.owner AND c_pk.constraint_name = b.constraint_name AND b.position = a.position \
             WHERE c.constraint_type = 'R' AND ROWNUM <= 400"
                .into(),
        ),
        _ => None,
    }
}

pub fn data_dictionary(state: &AppState, request: &ObjectRef) -> AppResult<String> {
    let tables = state.with_session(&request.connection_id, |session| {
        session.list_tables(&request.database, &request.schema)
    })?;
    let mut doc = format!(
        "# 数据字典\n\n库：{} / {}\n\n",
        request.database, request.schema
    );
    for table in tables.iter().take(60) {
        let columns = columns_of(
            state,
            &request.connection_id,
            &request.database,
            &request.schema,
            table,
        )?;
        doc.push_str(&format!(
            "## {table}\n\n| 列 | 类型 | 可空 | 主键 |\n| --- | --- | --- | --- |\n"
        ));
        for column in columns {
            doc.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                column.name,
                column.data_type,
                if column.nullable { "是" } else { "否" },
                if column.primary_key { "是" } else { "" }
            ));
        }
        doc.push('\n');
    }
    Ok(doc)
}

pub fn search_catalog(state: &AppState, request: &ObjectRef, query: &str) -> AppResult<GridResult> {
    let needle = query.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return Err(AppError::msg("输入要查找的名字"));
    }
    let tables = state.with_session(&request.connection_id, |session| {
        session.list_tables(&request.database, &request.schema)
    })?;
    let mut rows = Vec::new();
    for table in tables.iter().take(80) {
        if table.to_ascii_lowercase().contains(&needle) {
            rows.push(vec![
                JsonValue::String("对象".into()),
                JsonValue::String(table.clone()),
                JsonValue::String(String::new()),
            ]);
        }
        if rows.len() >= 200 {
            break;
        }
        let columns = columns_of(
            state,
            &request.connection_id,
            &request.database,
            &request.schema,
            table,
        )
        .unwrap_or_default();
        for column in columns {
            if column.name.to_ascii_lowercase().contains(&needle) {
                rows.push(vec![
                    JsonValue::String("列".into()),
                    JsonValue::String(table.clone()),
                    JsonValue::String(column.name),
                ]);
            }
        }
    }
    Ok(GridResult {
        columns: vec!["类型".into(), "对象".into(), "列".into()],
        rows,
        note: String::new(),
    })
}

pub(super) fn object_snap(
    state: &AppState,
    id: &str,
    database: &str,
    schema: &str,
    table: &str,
) -> AppResult<ObjectSnap> {
    let columns = columns_of(state, id, database, schema, table)?;
    let engine = engine_of(state, id)?;
    let indexes = load_indexes(state, id, database, schema, table, engine).unwrap_or_default();
    let foreign_keys = load_fks(state, id, database, schema, table, engine).unwrap_or_default();
    let view_sql = load_view_sql(state, id, database, schema, table, engine)
        .ok()
        .flatten();
    Ok(ObjectSnap {
        columns,
        indexes,
        foreign_keys,
        view_sql,
    })
}

fn load_indexes(
    state: &AppState,
    id: &str,
    database: &str,
    schema: &str,
    table: &str,
    engine: Engine,
) -> AppResult<Vec<IndexSnap>> {
    let sql = match engine {
        Engine::Mysql | Engine::Mariadb => {
            let db = sql_literal(engine, &JsonValue::String(if database.is_empty() { schema.to_string() } else { database.to_string() }))?;
            let table_lit = sql_literal(engine, &JsonValue::String(table.to_string()))?;
            format!("SELECT INDEX_NAME, COLUMN_NAME, NON_UNIQUE FROM information_schema.STATISTICS WHERE TABLE_SCHEMA = {db} AND TABLE_NAME = {table_lit} AND INDEX_NAME <> 'PRIMARY' ORDER BY INDEX_NAME, SEQ_IN_INDEX")
        }
        Engine::Postgres => {
            let schema_name = if schema.is_empty() { "public" } else { schema };
            let schema_lit = sql_literal(engine, &JsonValue::String(schema_name.to_string()))?;
            let table_lit = sql_literal(engine, &JsonValue::String(table.to_string()))?;
            format!("SELECT indexname, indexdef FROM pg_indexes WHERE schemaname = {schema_lit} AND tablename = {table_lit}")
        }
        Engine::Sqlite => {
            let quoted = quote_ident(Engine::Sqlite, table)?;
            format!("PRAGMA index_list({quoted})")
        }
        Engine::Mssql => format!(
            "SELECT i.name, c.name, i.is_unique FROM sys.indexes i JOIN sys.index_columns ic ON i.object_id = ic.object_id AND i.index_id = ic.index_id JOIN sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id WHERE i.object_id = OBJECT_ID({}) AND i.is_primary_key = 0 AND i.name IS NOT NULL ORDER BY i.name, ic.key_ordinal",
            sql_literal(engine, &JsonValue::String(table.to_string()))?
        ),
        _ => return Ok(Vec::new()),
    };
    let outcome = state.with_session(id, |session| session.execute(database, &sql, Some(400)))?;
    let mut indexes: Vec<IndexSnap> = Vec::new();
    if engine == Engine::Postgres {
        for row in outcome.rows {
            let name = cell_string(row.first());
            let def = cell_string(row.get(1));
            if name.is_empty() {
                continue;
            }
            indexes.push(IndexSnap {
                name,
                columns: columns_in_parens(&def),
                unique: def.to_ascii_lowercase().contains("unique"),
            });
        }
        return Ok(indexes);
    }
    if engine == Engine::Sqlite {
        for row in outcome.rows {
            let name = cell_string(row.get(1));
            let unique = cell_string(row.get(2)) == "1";
            if name.is_empty() {
                continue;
            }
            let info = state.with_session(id, |session| {
                let quoted = quote_ident(Engine::Sqlite, &name)?;
                session.execute(database, &format!("PRAGMA index_info({quoted})"), Some(32))
            })?;
            let columns = info
                .rows
                .iter()
                .map(|item| cell_string(item.get(2)))
                .filter(|item| !item.is_empty())
                .collect();
            indexes.push(IndexSnap {
                name,
                columns,
                unique,
            });
        }
        return Ok(indexes);
    }
    for row in outcome.rows {
        let name = cell_string(row.first());
        let column = cell_string(row.get(1));
        let unique = match engine {
            Engine::Mysql | Engine::Mariadb => cell_string(row.get(2)) == "0",
            _ => {
                cell_string(row.get(2)) == "1"
                    || cell_string(row.get(2)).eq_ignore_ascii_case("true")
            }
        };
        if let Some(existing) = indexes.iter_mut().find(|item| item.name == name) {
            existing.columns.push(column);
        } else if !name.is_empty() {
            indexes.push(IndexSnap {
                name,
                columns: vec![column],
                unique,
            });
        }
    }
    Ok(indexes)
}

fn columns_in_parens(definition: &str) -> Vec<String> {
    let Some(start) = definition.rfind('(') else {
        return Vec::new();
    };
    let Some(end) = definition.rfind(')') else {
        return Vec::new();
    };
    definition[start + 1..end]
        .split(',')
        .map(|part| part.trim().trim_matches('"').trim_matches('`').to_string())
        .filter(|part| !part.is_empty())
        .collect()
}

pub fn foreign_keys(
    state: &AppState,
    id: &str,
    database: &str,
    schema: &str,
    table: &str,
) -> AppResult<Vec<ForeignKeyRef>> {
    let engine = engine_of(state, id)?;
    let keys = load_fks(state, id, database, schema, table, engine)?;
    Ok(keys
        .into_iter()
        .filter_map(|item| {
            let column = item.columns.first().cloned().unwrap_or_default();
            let ref_column = item.ref_columns.first().cloned().unwrap_or_default();
            if column.is_empty() || item.ref_table.is_empty() || ref_column.is_empty() {
                None
            } else {
                Some(ForeignKeyRef {
                    column,
                    ref_schema: schema.to_string(),
                    ref_table: item.ref_table,
                    ref_column,
                })
            }
        })
        .collect())
}

pub fn reference_rows(
    state: &AppState,
    id: &str,
    database: &str,
    schema: &str,
    table: &str,
    column: &str,
    query: &str,
) -> AppResult<GridResult> {
    let engine = engine_of(state, id)?;
    if !engine.is_sql() {
        return Err(AppError::msg("文档库没有外键取值"));
    }
    let mut names = crate::schema::list_columns(state, id, database, schema, table)
        .map(|items| {
            items
                .into_iter()
                .filter(|item| !crate::sqlutil::is_binary_type(&item.data_type))
                .map(|item| item.name)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !names.iter().any(|name| name.eq_ignore_ascii_case(column)) {
        names.insert(0, column.to_string());
    }
    let borrowed = names.iter().map(String::as_str).collect::<Vec<_>>();
    let sql = crate::sqlutil::reference_sql(engine, database, schema, table, &borrowed, query, 40)?;
    let outcome = state.with_session(id, |session| session.execute(database, &sql, Some(40)))?;
    Ok(GridResult {
        columns: outcome
            .columns
            .iter()
            .map(|column| column.name.clone())
            .collect(),
        rows: outcome.rows,
        note: format!("最多 40 行，来自 {table}.{column}"),
    })
}

fn load_fks(
    state: &AppState,
    id: &str,
    database: &str,
    schema: &str,
    table: &str,
    engine: Engine,
) -> AppResult<Vec<FkSnap>> {
    if engine == Engine::Sqlite {
        let quoted = quote_ident(Engine::Sqlite, table)?;
        let outcome = state.with_session(id, |session| {
            session.execute(
                database,
                &format!("PRAGMA foreign_key_list({quoted})"),
                Some(100),
            )
        })?;
        return Ok(outcome
            .rows
            .into_iter()
            .map(|row| FkSnap {
                name: format!("fk_{}", cell_string(row.get(2))),
                columns: vec![cell_string(row.get(3))],
                ref_table: cell_string(row.get(2)),
                ref_columns: vec![cell_string(row.get(4))],
            })
            .filter(|item| !item.ref_table.is_empty())
            .collect());
    }
    let Some(sql) = fk_query(engine, database, schema) else {
        return Ok(Vec::new());
    };
    let outcome = state.with_session(id, |session| session.execute(database, &sql, Some(500)))?;
    Ok(outcome
        .rows
        .into_iter()
        .filter(|row| cell_string(row.first()).eq_ignore_ascii_case(table))
        .map(|row| FkSnap {
            name: format!(
                "fk_{}_{}",
                cell_string(row.first()),
                cell_string(row.get(1))
            ),
            columns: vec![cell_string(row.get(1))],
            ref_table: cell_string(row.get(2)),
            ref_columns: vec![cell_string(row.get(3))],
        })
        .filter(|item| !item.columns[0].is_empty() && !item.ref_table.is_empty())
        .collect())
}

fn load_view_sql(
    state: &AppState,
    id: &str,
    database: &str,
    schema: &str,
    table: &str,
    engine: Engine,
) -> AppResult<Option<String>> {
    let sql = match engine {
        Engine::Sqlite => format!(
            "SELECT sql FROM sqlite_master WHERE type = 'view' AND name = {}",
            sql_literal(engine, &JsonValue::String(table.to_string()))?
        ),
        Engine::Mysql | Engine::Mariadb => format!(
            "SELECT VIEW_DEFINITION FROM information_schema.VIEWS WHERE TABLE_SCHEMA = {} AND TABLE_NAME = {}",
            sql_literal(engine, &JsonValue::String(if database.is_empty() { schema.to_string() } else { database.to_string() }))?,
            sql_literal(engine, &JsonValue::String(table.to_string()))?
        ),
        Engine::Postgres => format!(
            "SELECT definition FROM pg_views WHERE schemaname = {} AND viewname = {}",
            sql_literal(engine, &JsonValue::String(if schema.is_empty() { "public".into() } else { schema.to_string() }))?,
            sql_literal(engine, &JsonValue::String(table.to_string()))?
        ),
        Engine::Mssql => format!(
            "SELECT definition FROM sys.sql_modules WHERE object_id = OBJECT_ID({})",
            sql_literal(engine, &JsonValue::String(table.to_string()))?
        ),
        _ => return Ok(None),
    };
    let outcome = state.with_session(id, |session| session.execute(database, &sql, Some(1)))?;
    Ok(outcome
        .rows
        .first()
        .and_then(|row| row.first())
        .and_then(|value| value.as_str())
        .map(|text| text.to_string())
        .filter(|text| !text.trim().is_empty()))
}
