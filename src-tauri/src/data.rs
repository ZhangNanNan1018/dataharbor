use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value as JsonValue;

pub(crate) mod delimited;

use crate::drivers::DbSession;
use crate::error::{AppError, AppResult};
use crate::models::{
    ColumnMeta, CommitRequest, CommitResult, DataPage, Engine, ExportResult, ExportSqlRequest,
    ExportTableRequest, FetchRowsRequest, FieldValue, FilterSpec, ImportCsvRequest, ImportResult,
    RowChange, SortSpec,
};
use crate::sqlutil::{
    json_count, like_contains, paginate_query_sql, paginate_sql, qualified_name, split_sql,
    sql_literal, value_sql,
};
use crate::state::AppState;

const EXPORT_CAP: u64 = 100_000;
const IMPORT_BYTES: u64 = 50 * 1024 * 1024;

pub fn fetch_rows(state: &AppState, request: FetchRowsRequest) -> AppResult<DataPage> {
    let page = request.page.max(1);
    let page_size = request.page_size.clamp(1, 500);
    state.with_session(&request.connection_id, |session| {
        if session.is_document_store() {
            let outcome = session.browse_page(
                &request.database,
                &request.schema,
                &request.table,
                &request.filters,
                &request.sorts,
                page_size,
                page_offset(page, page_size)?,
            )?;
            let total =
                session.count_filtered_relation(&request.database, &request.schema, &request.table, &request.filters)?;
            return Ok(DataPage {
                original_rows: outcome.original_rows,
                columns: outcome.columns,
                rows: outcome.rows,
                total,
                page,
                page_size,
                read_only: false,
            });
        }
        let engine = session.engine();
        let plan = plan_table(
            session,
            engine,
            &request.database,
            &request.schema,
            &request.table,
            &request.filters,
            &request.sorts,
        )?;
        let offset = page_offset(page, page_size)?;
        let sql = paginate_sql(engine, &plan.select_body, page_size, offset);
        let outcome = session.execute(&request.database, &sql, Some(page_size as usize))?;
        let count = session.execute(&request.database, &plan.count_sql, Some(1))?;
        let total = count
            .rows
            .first()
            .and_then(|row| row.first())
            .and_then(json_count)
            .unwrap_or(0);
        Ok(DataPage {
            original_rows: None,
            columns: plan.columns,
            rows: outcome.rows,
            total,
            page,
            page_size,
            read_only: plan.read_only,
        })
    })
}

pub fn commit_changes(state: &AppState, request: CommitRequest) -> AppResult<CommitResult> {
    if request.changes.is_empty() {
        return Err(AppError::msg("没有需要提交的更改"));
    }
    state.with_session(&request.connection_id, |session| {
        if session.is_document_store() {
            for (index, change) in request.changes.iter().enumerate() {
                session
                    .apply_mutation(&request.database, &request.schema, &request.table, change)
                    .map_err(|err| AppError::PartialWrite { applied: index, failed_index: index + 1, detail: err.to_string() })?;
            }
            return Ok(CommitResult {
                applied: request.changes.len(),
            });
        }
        let engine = session.engine();
        let columns = session.list_columns(&request.database, &request.schema, &request.table)?;
        if session
            .relation_kind(&request.database, &request.schema, &request.table)?
            .as_deref()
            == Some("view")
        {
            return Err(AppError::msg("视图是只读的"));
        }
        let statements = request
            .changes
            .iter()
            .enumerate()
            .map(|(index, change)| {
                let compiled = compile_change(
                    engine,
                    &request.database,
                    &request.schema,
                    &request.table,
                    &columns,
                    change,
                )
                .map_err(|err| AppError::msg(format!("第 {} 条更改无效: {err}", index + 1)))?;
                Ok(compiled.map(|sql| (sql, Some(1))))
            })
            .collect::<AppResult<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        let applied = session.execute_checked_transaction(&request.database, &statements)?;
        Ok(CommitResult {
            applied,
        })
    })
}

pub fn export_table_csv(state: &AppState, request: ExportTableRequest) -> AppResult<ExportResult> {
    state.with_session(&request.connection_id, |session| {
        if session.is_document_store() {
            let total =
                session.count_filtered_relation(&request.database, &request.schema, &request.table, &request.filters)?;
            let outcome = session.browse_page(
                &request.database,
                &request.schema,
                &request.table,
                &request.filters,
                &request.sorts,
                EXPORT_CAP as u32,
                0,
            )?;
            write_csv(&request.path, &outcome.columns, &outcome.rows)?;
            return Ok(ExportResult {
                path: request.path,
                row_count: outcome.rows.len() as u64,
                truncated: total > EXPORT_CAP,
            });
        }
        let engine = session.engine();
        let plan = plan_table(
            session,
            engine,
            &request.database,
            &request.schema,
            &request.table,
            &request.filters,
            &request.sorts,
        )?;
        let count = session.execute(&request.database, &plan.count_sql, Some(1))?;
        let total = count
            .rows
            .first()
            .and_then(|row| row.first())
            .and_then(json_count)
            .unwrap_or(0);
        let sql = paginate_sql(engine, &plan.select_body, EXPORT_CAP as u32, 0);
        let outcome = session.execute(&request.database, &sql, Some(EXPORT_CAP as usize))?;
        write_csv(&request.path, &plan.columns, &outcome.rows)?;
        Ok(ExportResult {
            path: request.path,
            row_count: outcome.rows.len() as u64,
            truncated: outcome.truncated || total > outcome.rows.len() as u64,
        })
    })
}

pub fn export_sql_csv(state: &AppState, request: ExportSqlRequest) -> AppResult<ExportResult> {
    let statements = split_sql(&request.sql);
    if statements.len() != 1 {
        return Err(AppError::msg("一次只导出一条 SQL"));
    }
    let sql = &statements[0];
    if !crate::sqlutil::is_read_query(sql) {
        return Err(AppError::msg("SQL 导出仅接受 SELECT 或只读 CTE 查询；写语句和含写操作的 CTE 已拒绝，避免导出时再次修改数据库"));
    }
    state.with_session(&request.connection_id, |session| {
        let read_limit = EXPORT_CAP as usize + 1;
        let mut outcome =
            if let Some(paged) = paginate_query_sql(session.engine(), sql, read_limit as u32, 0) {
                match session.execute(&request.database, &paged, Some(read_limit)) {
                    Ok(outcome) => outcome,
                    Err(err) => return Err(err),
                }
            } else {
                session.execute(&request.database, sql, Some(read_limit))?
            };
        if outcome.columns.is_empty() {
            return Err(AppError::msg("该语句没有可导出的结果集"));
        }
        let truncated = outcome.truncated || outcome.rows.len() as u64 > EXPORT_CAP;
        outcome.rows.truncate(EXPORT_CAP as usize);
        write_csv(&request.path, &outcome.columns, &outcome.rows)?;
        Ok(ExportResult {
            path: request.path,
            row_count: outcome.rows.len() as u64,
            truncated,
        })
    })
}

pub fn import_table_csv(state: &AppState, request: ImportCsvRequest) -> AppResult<ImportResult> {
    let meta =
        fs::metadata(&request.path).map_err(|err| AppError::msg(format!("无法读取 CSV: {err}")))?;
    if meta.len() > IMPORT_BYTES {
        return Err(AppError::msg("CSV 超过 50MB"));
    }
    let file = fs::File::open(&request.path).map_err(|err| AppError::msg(format!("无法读取 CSV: {err}")))?;
    let (headers, records) = delimited::read(file, b',', EXPORT_CAP as usize)?;

    state.with_session(&request.connection_id, |session| {
        let engine = session.engine();
        let columns = session.list_columns(&request.database, &request.schema, &request.table)?;
        if session
            .relation_kind(&request.database, &request.schema, &request.table)?
            .as_deref()
            == Some("view")
        {
            return Err(AppError::msg("不能向视图导入"));
        }
        let mapping = headers
            .iter()
            .map(|header| {
                columns
                    .iter()
                    .position(|column| column.name.eq_ignore_ascii_case(header))
            })
            .collect::<Vec<_>>();
        if mapping.iter().any(|index| index.is_none()) {
            return Err(AppError::msg("CSV 表头包含不存在的字段，已取消导入"));
        }
        if session.is_document_store() {
            return Err(AppError::msg(
                "CSV 事务导入目前仅支持 SQL 数据库；文档库请使用 JSON 导入",
            ));
        }
        let mut statements = Vec::new();
        if request.truncate {
            let table_sql =
                qualified_name(engine, &request.database, &request.schema, &request.table)?;
            statements.push(format!("DELETE FROM {table_sql}"));
        }
        for (index, record) in records.iter().enumerate() {
            let values = record
                .iter()
                .enumerate()
                .filter_map(|(header_index, cell)| {
                    let column_index = mapping.get(header_index).copied().flatten()?;
                    let column = &columns[column_index];
                    Some(FieldValue {
                        column: column.name.clone(),
                        value: cell.clone(),
                    })
                })
                .collect();
            let change = RowChange {
                kind: "insert".into(),
                original: None,
                pk: Vec::new(),
                values,
            };
            if let Some(sql) = compile_change(
                engine,
                &request.database,
                &request.schema,
                &request.table,
                &columns,
                &change,
            )
            .map_err(|err| AppError::msg(format!("第 {} 行 CSV 无效: {err}", index + 2)))?
            {
                statements.push(sql);
            }
        }
        // Validate the entire file, including binary values, before DELETE or INSERT.
        session.execute_transaction(&request.database, &statements)?;
        Ok(ImportResult {
            inserted: records.len() as u64,
        })
    })
}

struct TablePlan {
    columns: Vec<ColumnMeta>,
    select_body: String,
    count_sql: String,
    read_only: bool,
}

fn plan_table(
    session: &mut dyn DbSession,
    engine: Engine,
    database: &str,
    schema: &str,
    table: &str,
    filters: &[FilterSpec],
    sorts: &[SortSpec],
) -> AppResult<TablePlan> {
    if sorts.len() > 3 {
        return Err(AppError::msg("排序最多 3 列"));
    }
    let columns = session.list_columns(database, schema, table)?;
    if columns.is_empty() {
        return Err(AppError::msg("找不到该对象或它没有列"));
    }
    let read_only = session.relation_kind(database, schema, table)?.as_deref() == Some("view");
    let table_sql = qualified_name(engine, database, schema, table)?;
    let column_sql = columns
        .iter()
        .map(|column| crate::sqlutil::quote_ident(engine, &column.name))
        .collect::<AppResult<Vec<_>>>()?
        .join(", ");
    let where_sql = compile_where(engine, filters, &columns)?;
    let stable_sorts = stable_sort(sorts, &columns);
    let order_sql = compile_order(engine, &stable_sorts, &columns)?;
    Ok(TablePlan {
        select_body: format!("SELECT {column_sql} FROM {table_sql}{where_sql}{order_sql}"),
        count_sql: format!("SELECT COUNT(*) FROM {table_sql}{where_sql}"),
        columns,
        read_only,
    })
}

fn compile_where(
    engine: Engine,
    filters: &[FilterSpec],
    columns: &[ColumnMeta],
) -> AppResult<String> {
    if filters.is_empty() {
        return Ok(String::new());
    }
    if filters.len() > 12 {
        return Err(AppError::msg("筛选条件最多 12 条"));
    }
    let mut parts = Vec::new();
    for filter in filters {
        let column = find_column(columns, &filter.column)?;
        let ident = crate::sqlutil::quote_ident(engine, &column.name)?;
        let piece = match filter.op.as_str() {
            "eq" | "ne" | "gt" | "lt" | "gte" | "lte" => {
                let raw = filter
                    .value
                    .as_deref()
                    .ok_or_else(|| AppError::msg("筛选需要取值"))?;
                let literal = literal_for_column(engine, column, raw)?;
                let operator = match filter.op.as_str() {
                    "eq" => "=",
                    "ne" => "<>",
                    "gt" => ">",
                    "lt" => "<",
                    "gte" => ">=",
                    _ => "<=",
                };
                format!("{ident} {operator} {literal}")
            }
            "contains" => like_contains(engine, &ident, filter.value.as_deref().unwrap_or(""))?,
            "is_null" => format!("{ident} IS NULL"),
            "is_not_null" => format!("{ident} IS NOT NULL"),
            _ => return Err(AppError::msg("不支持的筛选运算符")),
        };
        parts.push(piece);
    }
    Ok(format!(" WHERE {}", parts.join(" AND ")))
}

fn compile_order(engine: Engine, sorts: &[SortSpec], columns: &[ColumnMeta]) -> AppResult<String> {
    if sorts.is_empty() {
        return Ok(String::new());
    }
    let mut parts = Vec::new();
    for sort in sorts {
        let column = find_column(columns, &sort.column)?;
        let ident = crate::sqlutil::quote_ident(engine, &column.name)?;
        let direction = if sort.desc { "DESC" } else { "ASC" };
        parts.push(format!("{ident} {direction}"));
    }
    Ok(format!(" ORDER BY {}", parts.join(", ")))
}

fn page_offset(page: u32, page_size: u32) -> AppResult<u32> {
    (page - 1)
        .checked_mul(page_size)
        .ok_or_else(|| AppError::msg("分页位置超出范围"))
}

// A unique tie-breaker prevents rows moving between pages when a user sorts on
// a non-unique column. Keep the user's ordering first.
fn stable_sort(sorts: &[SortSpec], columns: &[ColumnMeta]) -> Vec<SortSpec> {
    let mut stable = sorts.to_vec();
    for column in columns.iter().filter(|column| column.primary_key) {
        if !stable.iter().any(|sort| sort.column == column.name) {
            stable.push(SortSpec {
                column: column.name.clone(),
                desc: false,
            });
        }
    }
    stable
}

fn compile_change(
    engine: Engine,
    database: &str,
    schema: &str,
    table: &str,
    columns: &[ColumnMeta],
    change: &RowChange,
) -> AppResult<Option<String>> {
    let table_sql = qualified_name(engine, database, schema, table)?;
    let mut seen = BTreeSet::new();
    for field in &change.values {
        if !seen.insert(field.column.as_str()) {
            return Err(AppError::msg(format!("列 {} 被重复赋值", field.column)));
        }
    }
    match change.kind.as_str() {
        "insert" => {
            let mut names = Vec::new();
            let mut values = Vec::new();
            for field in &change.values {
                let column = find_column(columns, &field.column)?;
                if column.auto_increment && is_blank(&field.value) {
                    continue;
                }
                names.push(crate::sqlutil::quote_ident(engine, &column.name)?);
                values.push(value_sql(engine, &column.data_type, &field.value)?);
            }
            let sql = if names.is_empty() {
                match engine {
                    Engine::Mysql | Engine::Mariadb => {
                        format!("INSERT INTO {table_sql} () VALUES ()")
                    }
                    _ => format!("INSERT INTO {table_sql} DEFAULT VALUES"),
                }
            } else {
                format!(
                    "INSERT INTO {table_sql} ({}) VALUES ({})",
                    names.join(", "),
                    values.join(", ")
                )
            };
            Ok(Some(sql))
        }
        "update" => {
            ensure_pk(columns, &change.pk)?;
            if change.values.is_empty() {
                return Ok(None);
            }
            let mut sets = Vec::new();
            for field in &change.values {
                let column = find_column(columns, &field.column)?;
                let ident = crate::sqlutil::quote_ident(engine, &column.name)?;
                sets.push(format!(
                    "{ident} = {}",
                    value_sql(engine, &column.data_type, &field.value)?
                ));
            }
            let predicate = edit_predicate(engine, columns, change)?;
            Ok(Some(format!(
                "UPDATE {table_sql} SET {} WHERE {predicate}",
                sets.join(", ")
            )))
        }
        "delete" => {
            ensure_pk(columns, &change.pk)?;
            let predicate = edit_predicate(engine, columns, change)?;
            Ok(Some(format!("DELETE FROM {table_sql} WHERE {predicate}")))
        }
        _ => Err(AppError::msg("未知的更改类型")),
    }
}

fn ensure_pk(columns: &[ColumnMeta], pk: &[FieldValue]) -> AppResult<()> {
    let expected: BTreeSet<&str> = columns
        .iter()
        .filter(|column| column.primary_key)
        .map(|column| column.name.as_str())
        .collect();
    if expected.is_empty() {
        return Err(AppError::msg("该表没有主键，无法安全更新或删除"));
    }
    let given: BTreeSet<&str> = pk.iter().map(|field| field.column.as_str()).collect();
    if given != expected {
        return Err(AppError::msg("主键不完整，已拒绝更改"));
    }
    Ok(())
}

fn pk_predicate(engine: Engine, columns: &[ColumnMeta], pk: &[FieldValue]) -> AppResult<String> {
    let mut parts = Vec::new();
    for field in pk {
        let column = find_column(columns, &field.column)?;
        if !column.primary_key {
            return Err(AppError::msg("提交的键不是主键"));
        }
        let ident = crate::sqlutil::quote_ident(engine, &column.name)?;
        if field.value.is_null() {
            parts.push(format!("{ident} IS NULL"));
        } else {
            parts.push(format!("{ident} = {}", sql_literal(engine, &field.value)?));
        }
    }
    Ok(parts.join(" AND "))
}

fn edit_predicate(engine: Engine, columns: &[ColumnMeta], change: &RowChange) -> AppResult<String> {
    let original = change.original.as_ref().ok_or_else(|| AppError::msg("缺少读取时的原始行，无法检查并发冲突；请刷新后重试"))?;
    let names = original.iter().map(|field| field.column.as_str()).collect::<BTreeSet<_>>();
    let expected = columns.iter().map(|column| column.name.as_str()).collect::<BTreeSet<_>>();
    if names != expected || original.len() != columns.len() {
        return Err(AppError::msg("原始行字段与当前结构不一致，请刷新后重试"));
    }
    let mut parts = vec![pk_predicate(engine, columns, &change.pk)?];
    for field in original {
        let column = find_column(columns, &field.column)?;
        let ident = crate::sqlutil::quote_ident(engine, &field.column)?;
        if field.value.is_null() {
            parts.push(format!("{ident} IS NULL"));
        } else {
            let literal = value_sql(engine, &column.data_type, &field.value)?;
            // JSON/legacy text columns have no equality operator on some
            // engines. Compare their canonical textual representation.
            let kind = column.data_type.to_ascii_lowercase();
            let predicate = match engine {
                Engine::Postgres if matches!(kind.as_str(), "json" | "xml") => format!("CAST({ident} AS TEXT) = {literal}"),
                Engine::Mssql if matches!(kind.as_str(), "text" | "ntext" | "xml") => format!("CAST({ident} AS NVARCHAR(MAX)) = {literal}"),
                Engine::Mysql | Engine::Mariadb if !crate::sqlutil::is_numeric_type(&kind) && !crate::sqlutil::is_binary_type(&kind) => format!("BINARY {ident} = BINARY {literal}"),
                _ => format!("{ident} = {literal}"),
            };
            parts.push(predicate);
        }
    }
    Ok(parts.join(" AND "))
}

fn find_column<'a>(columns: &'a [ColumnMeta], name: &str) -> AppResult<&'a ColumnMeta> {
    columns
        .iter()
        .find(|column| column.name == name)
        .ok_or_else(|| AppError::msg(format!("未知列 {name}")))
}

fn literal_for_column(engine: Engine, column: &ColumnMeta, raw: &str) -> AppResult<String> {
    if is_bool_type(&column.data_type) {
        let lowered = raw.to_ascii_lowercase();
        if matches!(lowered.as_str(), "1" | "t" | "true" | "yes") {
            return sql_literal(engine, &JsonValue::Bool(true));
        }
        if matches!(lowered.as_str(), "0" | "f" | "false" | "no") {
            return sql_literal(engine, &JsonValue::Bool(false));
        }
    }
    value_sql(engine, &column.data_type, &JsonValue::String(raw.to_string()))
}

fn is_blank(value: &JsonValue) -> bool {
    value.is_null() || value.as_str().is_some_and(|text| text.is_empty())
}

fn is_bool_type(data_type: &str) -> bool {
    let data_type = data_type.to_ascii_lowercase();
    data_type.starts_with("bool")
}

fn write_csv(path: &str, columns: &[ColumnMeta], rows: &[Vec<JsonValue>]) -> AppResult<()> {
    if path.trim().is_empty() {
        return Err(AppError::msg("请选择导出路径"));
    }
    let path = Path::new(path);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|err| AppError::msg(format!("无法创建目录: {err}")))?;
        }
    }
    let mut buffer = vec![0xEF, 0xBB, 0xBF];
    let names = columns.iter().map(|column| column.name.clone()).collect::<Vec<_>>();
    delimited::write(&mut buffer, &names, rows, b',')?;
    fs::write(path, buffer).map_err(|err| AppError::msg(format!("写入 CSV 失败: {err}")))?;
    Ok(())
}

#[cfg(test)]
mod tests;
