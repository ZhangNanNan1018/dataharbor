use super::storage::engine_of;
use super::types::{BindRequest, DesignRequest, GridResult, SqlText};
use crate::error::{AppError, AppResult};
use crate::models::{Engine, ExecuteSqlRequest};
use crate::query::execute_sql;
use crate::sqlfmt::{self, substitute_params};
use crate::sqlutil::{qualified_name, quote_ident, sql_literal};
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
    let statement = state.with_session(connection_id, |session| {
        let engine = session.engine();
        let trimmed = sql.trim().trim_end_matches(';').trim();
        let plan = match engine {
            Engine::Mysql | Engine::Mariadb => format!("EXPLAIN FORMAT=JSON {trimmed}"),
            Engine::Postgres => format!("EXPLAIN (FORMAT JSON) {trimmed}"),
            Engine::Sqlite => format!("EXPLAIN QUERY PLAN {trimmed}"),
            Engine::Mssql => format!("SET SHOWPLAN_ALL ON; {trimmed}"),
            Engine::Oracle => format!("EXPLAIN PLAN FOR {trimmed}"),
            Engine::Snowflake => format!("EXPLAIN {trimmed}"),
            Engine::Dameng => format!("EXPLAIN {trimmed}"),
            Engine::Redis | Engine::Mongodb => {
                return Err(AppError::msg("文档库没有 SQL 执行计划。Redis 用 INFO / COMMAND，MongoDB 用 explain 命令 JSON。"));
            }
        };
        Ok(plan)
    })?;
    let response = execute_sql(
        state,
        ExecuteSqlRequest {
            connection_id: connection_id.to_string(),
            database: database.to_string(),
            sql: statement,
            page: 1,
            page_size: 200,
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
            first.error.unwrap_or_else(|| "执行计划失败".into()),
        ));
    }
    Ok(GridResult {
        columns: first
            .columns
            .iter()
            .map(|column| column.name.clone())
            .collect(),
        rows: first.rows,
        note: "执行计划来自数据库自身。SQL Server 若拒绝 SHOWPLAN，请改用具有相应权限的账号。"
            .into(),
    })
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
        ExecuteSqlRequest {
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
