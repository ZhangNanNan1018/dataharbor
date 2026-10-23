use super::storage::engine_of;
use super::types::{BindRequest, DesignRequest, GridResult, SqlText};
use crate::drivers::DbSession;
use crate::error::{AppError, AppResult};
use crate::models::{Engine, ExecuteSqlRequest, ExecuteSqlResponse, QueryOutcome, StatementResult};
use crate::operations::OperationContext;
use crate::query::execute_sql;
use crate::sqlfmt::{self, substitute_params};
use crate::sqlutil::{is_read_query, paginate_query_sql, qualified_name, quote_ident, split_sql, sql_literal};
use crate::state::AppState;
use serde_json::Value as JsonValue;

pub fn format_sql(request: SqlText) -> String {
    sqlfmt::format_sql(&request.sql, request.minify)
}

pub fn bind_sql(request: BindRequest) -> AppResult<String> {
    let pairs = request
        .params
        .iter()
        .map(|item| {
            sql_literal(request.engine, &item.value).map(|literal| (item.name.clone(), literal))
        })
        .collect::<AppResult<Vec<_>>>()?;
    Ok(substitute_params(&request.sql, &pairs))
}

pub fn explain_query(
    state: &AppState,
    connection_id: &str,
    database: &str,
    sql: &str,
) -> AppResult<GridResult> {
    let statements = split_sql(sql);
    if statements.len() != 1 {
        return Err(AppError::msg("一次只能查看一条语句的执行计划"));
    }
    // SHOWPLAN changes connection state. Never install it on the editor's
    // persistent session, and retire this connection even when cleanup fails.
    let mut session = state.open_isolated_session(connection_id)?;
    let outcome = explain_on_session(&mut *session, database, &statements[0])?;
    if outcome.columns.is_empty() {
        return Err(AppError::msg("当前驱动没有返回执行计划结果，请使用数据库原生客户端查看"));
    }
    Ok(GridResult {
        columns: outcome.columns.into_iter().map(|column| column.name).collect(),
        rows: outcome.rows,
        note: "执行计划来自独立数据库会话；SQL Server SHOWPLAN 设置不会影响查询编辑器。".into(),
    })
}

fn explain_on_session(session: &mut dyn DbSession, database: &str, sql: &str) -> AppResult<QueryOutcome> {
    if session.engine() == Engine::Mssql {
        let result = session.execute(database, "SET SHOWPLAN_ALL ON", None)
            .and_then(|_| session.execute(database, sql, Some(5000)));
        let reset = session.execute(database, "SET SHOWPLAN_ALL OFF", None);
        return match (result, reset) {
            (result, Ok(_)) => result,
            (Ok(_), Err(error)) => Err(AppError::msg(format!("执行计划会话清理失败，独立会话已关闭：{error}"))),
            (Err(error), Err(cleanup)) => Err(AppError::msg(format!("执行计划失败：{error}；独立会话清理失败：{cleanup}"))),
        };
    }
    let plan = match session.engine() {
            Engine::Mysql | Engine::Mariadb => format!("EXPLAIN FORMAT=JSON {sql}"),
            Engine::Postgres => format!("EXPLAIN (FORMAT JSON) {sql}"),
            Engine::Sqlite => format!("EXPLAIN QUERY PLAN {sql}"),
            Engine::Oracle => format!("EXPLAIN PLAN FOR {sql}"),
            Engine::Snowflake | Engine::Dameng => format!("EXPLAIN {sql}"),
            Engine::Mssql => unreachable!(),
            Engine::Redis | Engine::Mongodb => {
                return Err(AppError::msg("文档库没有 SQL 执行计划。Redis 用 INFO / COMMAND，MongoDB 用 explain 命令 JSON。"));
            }
    };
    session.execute(database, &plan, Some(5000))
}

/// BI never reaches the unrestricted SQL executor. Syntax validation happens
/// before opening a connection; supported engines additionally enforce a
/// read-only transaction, including functions invoked from SELECT.
pub fn execute_read_query_with_operation(
    state: &AppState,
    request: ExecuteSqlRequest,
    operation: &OperationContext,
) -> AppResult<ExecuteSqlResponse> {
    operation.checkpoint()?;
    let sql = read_query_statement(&request.sql)?;
    if request.only_statement.is_some_and(|index| index != 0) {
        return Err(AppError::msg("只读查询只接受一条语句"));
    }
    let mut session = state.open_isolated_session(&request.connection_id)?;
    if !session.engine().is_sql() {
        return Err(AppError::msg("BI 看板只接受 SQL SELECT 或只读 CTE 查询"));
    }
    let page = request.page.max(1);
    let page_size = request.page_size.clamp(1, 1000);
    let offset = (page - 1).checked_mul(page_size).ok_or_else(|| AppError::msg("分页位置超出范围"))?;
    let paged = paginate_query_sql(session.engine(), &sql, page_size + 1, offset);
    if paged.is_none() && page > 1 {
        return Err(AppError::msg("这条只读语句不支持自动分页"));
    }
    let enforced = session.engine().capabilities().read_snapshot;
    operation.report("执行只读查询", 0, None, if enforced { "独立只读事务；取消不会影响查询编辑器" } else { "已校验 SELECT/只读 CTE 语法；该引擎仍需避免调用有副作用的函数" });
    let interrupt = operation.bind_interrupt(session.cancel_handle(&request.database)?)?;
    if enforced {
        session.begin_read_snapshot(&request.database)?;
    }
    let started = std::time::Instant::now();
    let result = operation.checkpoint().and_then(|_| session.execute(&request.database, paged.as_deref().unwrap_or(&sql), Some(page_size as usize + 1)));
    // A late interrupt must never race cleanup or another task's connection.
    drop(interrupt);
    let ended = if enforced { session.end_read_snapshot(&request.database) } else { Ok(()) };
    operation.checkpoint()?;
    let mut outcome = result?;
    ended?;
    if paged.is_some() && outcome.truncated && outcome.rows.len() <= page_size as usize {
        return Err(AppError::msg("当前页结果超过读取预算，请减少查询字段或行数；分页已停止以避免遗漏记录"));
    }
    let has_more = outcome.truncated || outcome.rows.len() > page_size as usize;
    outcome.rows.truncate(page_size as usize);
    let total_rows = if paged.is_some() && !has_more { Some(u64::from(offset) + outcome.rows.len() as u64) } else { None };
    operation.report("只读查询已完成", outcome.rows.len() as u64, Some(outcome.rows.len() as u64), if enforced { "独立只读事务已结束" } else { "已通过语法只读校验；SELECT 函数的副作用由数据库定义" });
    Ok(ExecuteSqlResponse { statements: vec![StatementResult {
        index: 0, sql, ok: true, error: None, columns: outcome.columns, rows: outcome.rows,
        rows_affected: 0, total_rows, page, page_size, pageable: paged.is_some(),
        elapsed_ms: started.elapsed().as_millis() as u64,
        message: if has_more { "结果尚有更多数据".into() } else { "只读查询已完成".into() },
        truncated: has_more, has_more,
    }], stopped: false })
}

fn read_query_statement(sql: &str) -> AppResult<String> {
    let mut statements = split_sql(sql);
    if statements.len() != 1 || !is_read_query(&statements[0]) {
        return Err(AppError::msg("BI 看板仅接受一条 SELECT 或只读 CTE 查询；多语句和写操作已拒绝"));
    }
    Ok(statements.remove(0))
}

pub fn explain_plan(
    state: &AppState,
    connection_id: &str,
    database: &str,
    sql: &str,
) -> AppResult<crate::plan::PlanNode> {
    let engine = engine_of(state, connection_id)?;
    let grid = explain_query(state, connection_id, database, sql)?;
    Ok(crate::plan::parse_plan(
        engine_key(engine),
        &grid.columns,
        &grid.rows,
    ))
}

fn engine_key(engine: Engine) -> &'static str {
    match engine {
        Engine::Mysql => "mysql",
        Engine::Mariadb => "mariadb",
        Engine::Postgres => "postgres",
        Engine::Sqlite => "sqlite",
        Engine::Mongodb => "mongodb",
        Engine::Redis => "redis",
        Engine::Mssql => "mssql",
        Engine::Oracle => "oracle",
        Engine::Snowflake => "snowflake",
        Engine::Dameng => "dameng",
    }
}

pub fn list_accounts(
    state: &AppState,
    connection_id: &str,
    database: &str,
) -> AppResult<GridResult> {
    let engine = engine_of(state, connection_id)?;
    let sql = match engine {
        Engine::Mysql | Engine::Mariadb => "SELECT user, host FROM mysql.user",
        Engine::Postgres => "SELECT usename, usesuper FROM pg_user",
        Engine::Mssql => "SELECT name, type_desc FROM sys.database_principals",
        Engine::Oracle => "SELECT username, account_status FROM all_users WHERE ROWNUM <= 200",
        Engine::Sqlite => return Ok(note_grid("SQLite 文件没有服务端账号")),
        Engine::Redis => "ACL LIST",
        Engine::Mongodb => "{ \"usersInfo\": 1 }",
        Engine::Snowflake => "SHOW USERS",
        Engine::Dameng => "SELECT USERNAME FROM ALL_USERS WHERE ROWNUM <= 200",
    };
    run_grid(
        state,
        connection_id,
        database,
        sql,
        "账号查询需要相应权限。失败时请改用有权限的用户。",
    )
}

pub fn server_activity(
    state: &AppState,
    connection_id: &str,
    database: &str,
) -> AppResult<GridResult> {
    let engine = engine_of(state, connection_id)?;
    let sql = match engine {
        Engine::Mysql | Engine::Mariadb => "SHOW FULL PROCESSLIST",
        Engine::Postgres => "SELECT pid, usename, state, left(query, 180) AS query FROM pg_stat_activity",
        Engine::Mssql => "SELECT session_id, status, command, login_name FROM sys.dm_exec_sessions",
        Engine::Oracle => "SELECT sid, serial#, username, status FROM v$session WHERE ROWNUM <= 100",
        Engine::Sqlite => return Ok(note_grid("SQLite 没有服务端会话列表")),
        Engine::Redis => "INFO",
        Engine::Mongodb => "{ \"serverStatus\": 1 }",
        Engine::Snowflake => "SELECT CURRENT_USER() AS user, CURRENT_ROLE() AS role, CURRENT_WAREHOUSE() AS warehouse",
        Engine::Dameng => "SELECT SESS_ID, USER_NAME, STATE FROM V$SESSIONS WHERE ROWNUM <= 100",
    };
    run_grid(
        state,
        connection_id,
        database,
        sql,
        "监控视图依赖服务器权限。",
    )
}

pub fn preview_design(request: &DesignRequest) -> AppResult<String> {
    let engine = request.engine;
    let name = qualified_name(engine, &request.database, &request.schema, &request.name)?;
    match request.kind.as_str() {
        "view" => {
            let body = if request.body.trim().is_empty() {
                "SELECT 1".into()
            } else {
                request.body.trim().trim_end_matches(';').to_string()
            };
            Ok(format!("CREATE VIEW {name} AS\n{body};"))
        }
        "index" => {
            let cols = request
                .columns
                .iter()
                .map(|column| quote_ident(engine, &column.name))
                .collect::<AppResult<Vec<_>>>()?
                .join(", ");
            if cols.is_empty() {
                return Err(AppError::msg("索引至少需要一列"));
            }
            let index = quote_ident(engine, &format!("idx_{}", request.name))?;
            Ok(format!("CREATE INDEX {index} ON {name} ({cols});"))
        }
        "procedure" => {
            let body = if request.body.trim().is_empty() {
                "BEGIN\n  SELECT 1;\nEND"
            } else {
                request.body.trim()
            };
            let proc = quote_ident(engine, &request.name)?;
            let sql = match engine {
                Engine::Mysql | Engine::Mariadb => format!("CREATE PROCEDURE {proc}()\n{body};"),
                Engine::Postgres => format!(
                    "CREATE OR REPLACE PROCEDURE {proc}()\nLANGUAGE plpgsql\nAS $$\n{body}\n$$;"
                ),
                Engine::Mssql => format!("CREATE PROCEDURE {proc}\nAS\n{body};"),
                Engine::Oracle => format!("CREATE OR REPLACE PROCEDURE {proc} AS\n{body};\n/"),
                _ => format!("-- {} 没有通用存储过程语法\n{body}", engine.label()),
            };
            Ok(sql)
        }
        _ => {
            if request.columns.is_empty() {
                return Err(AppError::msg("表至少需要一列"));
            }
            let defs = request
                .columns
                .iter()
                .map(|column| {
                    let quoted = quote_ident(engine, &column.name)?;
                    let data_type = if column.data_type.trim().is_empty() {
                        "TEXT".to_string()
                    } else {
                        column.data_type.trim().to_string()
                    };
                    let null_sql = if column.nullable { "" } else { " NOT NULL" };
                    let pk = if column.primary_key {
                        " PRIMARY KEY"
                    } else {
                        ""
                    };
                    Ok(format!("  {quoted} {data_type}{null_sql}{pk}"))
                })
                .collect::<AppResult<Vec<_>>>()?
                .join(",\n");
            Ok(format!("CREATE TABLE {name} (\n{defs}\n);"))
        }
    }
}

pub(super) fn run_grid(
    state: &AppState,
    connection_id: &str,
    database: &str,
    sql: &str,
    note: &str,
) -> AppResult<GridResult> {
    let response = execute_sql(
        state,
        ExecuteSqlRequest { session_id: None,
            connection_id: connection_id.to_string(),
            database: database.to_string(),
            sql: sql.to_string(),
            page: 1,
            page_size: 100,
            only_statement: None,
        },
    )?;
    let first = response
        .statements
        .into_iter()
        .next()
        .ok_or_else(|| AppError::msg("没有结果"))?;
    if !first.ok {
        return Ok(note_grid(&format!(
            "{note} {}",
            first.error.unwrap_or_default()
        )));
    }
    Ok(GridResult {
        columns: first
            .columns
            .iter()
            .map(|column| column.name.clone())
            .collect(),
        rows: first.rows,
        note: note.into(),
    })
}

pub(super) fn note_grid(note: &str) -> GridResult {
    GridResult {
        columns: vec!["说明".into()],
        rows: vec![vec![JsonValue::String(note.into())]],
        note: note.into(),
    }
}

#[cfg(test)]
#[path = "query_tools_tests.rs"]
mod tests;
