use std::sync::{Mutex, MutexGuard};
use std::thread::JoinHandle;

use ::postgres::Client;
use serde::Serialize;
use serde_json::Value as JsonValue;

use crate::drivers::postgres::{self, run_simple};
use crate::error::{AppError, AppResult};
use crate::models::Engine;
use crate::sqlutil::sql_literal;
use crate::state::AppState;

/// Control connection plus the target thread that is blocked inside the function.
pub struct DebugSlot {
    control: Mutex<Client>,
    session_id: i64,
    function: String,
    target: Mutex<Option<JoinHandle<String>>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugCell {
    pub name: String,
    pub value: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugSnapshot {
    pub status: String,
    pub function: String,
    pub line: Option<i64>,
    pub message: String,
    pub variables: Vec<DebugCell>,
    pub stack: Vec<DebugCell>,
    pub target_note: String,
}

pub fn sql_listener() -> &'static str {
    "SELECT pldbg_create_listener()"
}

pub fn sql_extension() -> &'static str {
    "SELECT extname FROM pg_extension WHERE extname = 'pldbgapi'"
}

pub fn sql_find_proc(function: &str) -> AppResult<String> {
    let function = function.trim().trim_matches('"');
    if function.is_empty() || function.contains(';') {
        return Err(AppError::msg("请填写函数名，例如 public.add_book"));
    }
    let (schema, name) = match function.rsplit_once('.') {
        Some((schema, name)) if !schema.is_empty() && !name.is_empty() => {
            (Some(schema.trim_matches('"')), name.trim_matches('"'))
        }
        _ => (None, function),
    };
    let name_lit = sql_literal(Engine::Postgres, &JsonValue::String(name.to_string()))?;
    let schema_sql = if let Some(schema) = schema {
        let literal = sql_literal(Engine::Postgres, &JsonValue::String(schema.to_string()))?;
        format!(" AND n.nspname = {literal}")
    } else {
        String::new()
    };
    Ok(format!(
        "SELECT p.oid::text FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace WHERE p.proname = {name_lit}{schema_sql} ORDER BY p.oid LIMIT 1"
    ))
}

pub fn sql_breakpoint(session: i64, oid: i64, line: i32) -> String {
    format!("SELECT pldbg_set_global_breakpoint({session}, {oid}, {line}, NULL)")
}

pub fn sql_wait(session: i64) -> String {
    format!("SELECT * FROM pldbg_wait_for_target({session})")
}

pub fn sql_step(session: i64, action: &str) -> AppResult<String> {
    let func = match action {
        "over" | "step_over" | "step" => "pldbg_step_over",
        "into" | "step_into" => "pldbg_step_into",
        "continue" | "cont" => "pldbg_continue",
        "wait" => "pldbg_wait_for_target",
        other => return Err(AppError::msg(format!("未知调试动作 {other}"))),
    };
    Ok(format!("SELECT * FROM {func}({session})"))
}

pub fn sql_variables(session: i64) -> String {
    format!("SELECT * FROM pldbg_get_variables({session})")
}

pub fn sql_stack(session: i64) -> String {
    format!("SELECT * FROM pldbg_get_stack({session})")
}

pub fn sql_abort(session: i64) -> String {
    format!("SELECT pldbg_abort_target({session})")
}

pub fn launch(
    state: &AppState,
    connection_id: &str,
    database: &str,
    function: &str,
    call_sql: &str,
    line: i32,
) -> AppResult<DebugSnapshot> {
    let config = state.live_config(connection_id)?;
    if config.engine != Engine::Postgres {
        return Err(AppError::msg("交互式单步只覆盖已安装 pldbgapi 的 PostgreSQL。MySQL 和 Oracle 没有可用协议。"));
    }
    discard(state, connection_id);
    let database = if database.trim().is_empty() {
        config.database.clone()
    } else {
        database.trim().to_string()
    };
    let call_sql = call_sql.trim().to_string();
    if call_sql.is_empty() {
        return Err(AppError::msg("请填写目标会话要执行的调用，例如 SELECT public.add_book(1)"));
    }
    let mut control = postgres::open_configured(&config, &database)?;
    let extension = run_simple(&mut control, sql_extension(), Some(5))?;
    let installed = extension.rows.iter().any(|row| {
        row.first().and_then(|value| value.as_str()) == Some("pldbgapi")
    });
    if !installed {
        return Err(AppError::msg(
            "未安装 pldbgapi。CREATE EXTENSION pldbgapi 需要超级用户，本程序不会自动安装。",
        ));
    }
    let listener = run_simple(&mut control, sql_listener(), Some(1))?;
    let session_id = first_i64(&listener).ok_or_else(|| AppError::msg("pldbg_create_listener 没有返回会话号"))?;
    let proc = run_simple(&mut control, &sql_find_proc(function)?, Some(1))?;
    let oid = first_i64(&proc).ok_or_else(|| AppError::msg(format!("找不到函数 {function}")))?;
    run_simple(&mut control, &sql_breakpoint(session_id, oid, line), Some(1))?;

    let target_config = config.clone();
    let target_db = database.clone();
    let target_sql = call_sql.clone();
    let handle = std::thread::spawn(move || match postgres::open_configured(&target_config, &target_db) {
        Ok(mut client) => match run_simple(&mut client, &target_sql, Some(20)) {
            Ok(outcome) => format!(
                "目标会话返回 {} 行，影响 {} 行。",
                outcome.rows.len(),
                outcome.rows_affected
            ),
            Err(err) => format!("目标会话：{err}"),
        },
        Err(err) => format!("目标会话无法连接：{err}"),
    });

    let waited = wait_on(&mut control, session_id);
    let function = function.trim().to_string();
    let mut snapshot = match waited {
        Ok(grid) => paused_snapshot(&function, session_id, &grid, &mut control),
        Err(err) if is_timeout(&err) => DebugSnapshot {
            status: "timeout".into(),
            function: function.clone(),
            line: None,
            message: "控制会话等了 20 秒，目标还没有停在断点。确认调用会进入该函数，并且行号是可执行行。可以再按“等待”。".into(),
            variables: Vec::new(),
            stack: Vec::new(),
            target_note: String::new(),
        },
        Err(err) => return Err(err),
    };
    let slot = DebugSlot {
        control: Mutex::new(control),
        session_id,
        function,
        target: Mutex::new(Some(handle)),
    };
    snapshot.target_note = target_note(&slot);
    lock_map(state).insert(connection_id.to_string(), slot);
    Ok(snapshot)
}

pub fn step(state: &AppState, connection_id: &str, action: &str) -> AppResult<DebugSnapshot> {
    let action = action.trim().to_ascii_lowercase();
    if action == "stop" || action == "abort" {
        return stop(state, connection_id);
    }
    let map = lock_map(state);
    let slot = map
        .get(connection_id)
        .ok_or_else(|| AppError::msg("没有正在进行的调试。请先开始。"))?;
    let mut control = lock_mutex(&slot.control);
    let sql = sql_step(slot.session_id, &action)?;
    let session_id = slot.session_id;
    let function = slot.function.clone();
    let waited = wait_sql(&mut control, &sql);
    let mut snapshot = match waited {
        Ok(grid) => paused_snapshot(&function, session_id, &grid, &mut control),
        Err(err) if is_timeout(&err) => DebugSnapshot {
            status: "timeout".into(),
            function,
            line: None,
            message: "这一步在 20 秒内没有返回。目标可能仍在运行。可以再按“等待”或“中止”。".into(),
            variables: Vec::new(),
            stack: Vec::new(),
            target_note: String::new(),
        },
        Err(err) => return Err(err),
    };
    drop(control);
    snapshot.target_note = target_note(slot);
    Ok(snapshot)
}

pub fn stop(state: &AppState, connection_id: &str) -> AppResult<DebugSnapshot> {
    let slot = lock_map(state)
        .remove(connection_id)
        .ok_or_else(|| AppError::msg("没有正在进行的调试"))?;
    let note = abort_slot(slot);
    Ok(DebugSnapshot {
        status: "stopped".into(),
        function: String::new(),
        line: None,
        message: "已中止目标并关闭控制会话。".into(),
        variables: Vec::new(),
        stack: Vec::new(),
        target_note: note,
    })
}

pub fn discard(state: &AppState, connection_id: &str) {
    if let Some(slot) = lock_map(state).remove(connection_id) {
        let _ = abort_slot(slot);
    }
}

fn abort_slot(slot: DebugSlot) -> String {
    let mut control = lock_mutex(&slot.control);
    let _ = run_simple(&mut control, &sql_abort(slot.session_id), Some(1));
    drop(control);
    let handle = lock_mutex(&slot.target).take();
    match handle {
        Some(handle) => handle.join().unwrap_or_else(|_| "目标线程已中断".into()),
        None => "目标会话已结束。".into(),
    }
}

fn wait_on(client: &mut Client, session: i64) -> AppResult<crate::models::QueryOutcome> {
    wait_sql(client, &sql_wait(session))
}

fn wait_sql(client: &mut Client, sql: &str) -> AppResult<crate::models::QueryOutcome> {
    let _ = run_simple(client, "SET statement_timeout = '20s'", None);
    let result = run_simple(client, sql, Some(20));
    let _ = run_simple(client, "SET statement_timeout = 0", None);
    result
}

fn paused_snapshot(
    function: &str,
    session: i64,
    grid: &crate::models::QueryOutcome,
    client: &mut Client,
) -> DebugSnapshot {
    let line = column_i64(grid, &["linenumber", "line"]);
    let variables = match run_simple(client, &sql_variables(session), Some(200)) {
        Ok(grid) => rows_as_cells(&grid, &["name"], &["value"]),
        Err(err) => vec![DebugCell {
            name: "变量".into(),
            value: String::new(),
            detail: err.to_string(),
        }],
    };
    let stack = match run_simple(client, &sql_stack(session), Some(50)) {
        Ok(grid) => rows_as_cells(&grid, &["targetname", "func"], &["linenumber", "line"]),
        Err(err) => vec![DebugCell {
            name: "栈".into(),
            value: String::new(),
            detail: err.to_string(),
        }],
    };
    DebugSnapshot {
        status: "paused".into(),
        function: function.to_string(),
        line,
        message: format!("已停在 {}。控制会话 {} 与目标会话是两条连接。", function, session),
        variables,
        stack,
        target_note: String::new(),
    }
}

fn rows_as_cells(grid: &crate::models::QueryOutcome, name_keys: &[&str], value_keys: &[&str]) -> Vec<DebugCell> {
    grid.rows
        .iter()
        .map(|row| {
            let name = named(grid, row, name_keys).unwrap_or_else(|| cell_text(row.first()));
            let value = named(grid, row, value_keys).unwrap_or_else(|| cell_text(row.last()));
            let detail = grid
                .columns
                .iter()
                .enumerate()
                .map(|(index, column)| format!("{}={}", column.name, cell_text(row.get(index))))
                .collect::<Vec<_>>()
                .join(" · ");
            DebugCell { name, value, detail }
        })
        .collect()
}

fn named(grid: &crate::models::QueryOutcome, row: &[JsonValue], keys: &[&str]) -> Option<String> {
    let index = grid.columns.iter().position(|column| {
        keys.iter().any(|key| column.name.eq_ignore_ascii_case(key))
    })?;
    let text = cell_text(row.get(index));
    if text.is_empty() { None } else { Some(text) }
}

fn column_i64(grid: &crate::models::QueryOutcome, keys: &[&str]) -> Option<i64> {
    let index = grid.columns.iter().position(|column| {
        keys.iter().any(|key| column.name.eq_ignore_ascii_case(key))
    })?;
    grid.rows.first().and_then(|row| row.get(index)).and_then(value_i64)
}

fn first_i64(grid: &crate::models::QueryOutcome) -> Option<i64> {
    grid.rows.first().and_then(|row| row.first()).and_then(value_i64)
}

fn value_i64(value: &JsonValue) -> Option<i64> {
    match value {
        JsonValue::Number(number) => number.as_i64().or_else(|| number.as_f64().map(|item| item as i64)),
        JsonValue::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

fn cell_text(value: Option<&JsonValue>) -> String {
    match value {
        Some(JsonValue::String(text)) => text.clone(),
        Some(JsonValue::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

fn target_note(slot: &DebugSlot) -> String {
    let mut handle = lock_mutex(&slot.target);
    let Some(join) = handle.as_ref() else {
        return "目标会话已结束。".into();
    };
    if !join.is_finished() {
        return "目标会话仍在断点处等待，或正在执行调用。".into();
    }
    handle.take().unwrap().join().unwrap_or_else(|_| "目标线程已中断".into())
}

fn is_timeout(err: &AppError) -> bool {
    let text = err.to_string().to_ascii_lowercase();
    text.contains("statement timeout") || text.contains("canceling statement")
}

fn lock_map(state: &AppState) -> MutexGuard<'_, std::collections::HashMap<String, DebugSlot>> {
    state
        .debuggers()
        .lock()
        .unwrap_or_else(|err| err.into_inner())
}

fn lock_mutex<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builders_cover_both_sessions() {
        assert_eq!(sql_listener(), "SELECT pldbg_create_listener()");
        let find = sql_find_proc("public.add_book").unwrap();
        assert!(find.contains("proname"));
        assert!(find.contains("'add_book'"));
        assert!(find.contains("'public'"));
        assert_eq!(
            sql_breakpoint(7, 42, -1),
            "SELECT pldbg_set_global_breakpoint(7, 42, -1, NULL)"
        );
        assert_eq!(sql_wait(7), "SELECT * FROM pldbg_wait_for_target(7)");
        assert_eq!(sql_step(7, "step_over").unwrap(), "SELECT * FROM pldbg_step_over(7)");
        assert_eq!(sql_step(7, "into").unwrap(), "SELECT * FROM pldbg_step_into(7)");
        assert_eq!(sql_step(7, "continue").unwrap(), "SELECT * FROM pldbg_continue(7)");
        assert_eq!(sql_variables(7), "SELECT * FROM pldbg_get_variables(7)");
        assert_eq!(sql_stack(7), "SELECT * FROM pldbg_get_stack(7)");
        assert_eq!(sql_abort(7), "SELECT pldbg_abort_target(7)");
        assert!(sql_find_proc("").is_err());
    }
}
