use super::query_tools::run_grid;
use super::storage::engine_of;
use super::types::DebugReport;
use crate::drivers::redis;
use crate::error::{AppError, AppResult};
use crate::models::Engine;
use crate::state::AppState;

pub fn redis_publish(
    state: &AppState,
    connection_id: &str,
    channel: &str,
    message: &str,
) -> AppResult<i64> {
    let config = state.session_config_for(connection_id)?;
    if config.engine != Engine::Redis {
        return Err(AppError::msg("只有 Redis 连接可以发布"));
    }
    redis::publish(&config, channel, message)
}

pub fn redis_listen(
    state: &AppState,
    connection_id: &str,
    channel: &str,
    wait_ms: u64,
) -> AppResult<Vec<String>> {
    let config = state.session_config_for(connection_id)?;
    if config.engine != Engine::Redis {
        return Err(AppError::msg("只有 Redis 连接可以订阅"));
    }
    if config.server_mode.eq_ignore_ascii_case("cluster") {
        return Err(AppError::msg("Cluster 模式请直接连接一个节点再订阅"));
    }
    redis::read_pubsub(&config, channel, wait_ms.clamp(200, 8000))
}

pub fn debug_status(
    state: &AppState,
    connection_id: &str,
    database: &str,
) -> AppResult<DebugReport> {
    let engine = engine_of(state, connection_id)?;
    match engine {
        Engine::Postgres => {
            let grid = run_grid(
                state,
                connection_id,
                database,
                "SELECT extname FROM pg_extension WHERE extname IN ('pldbgapi', 'plpgsql_check')",
                "需要权限读取 pg_extension。",
            )?;
            let names = grid
                .rows
                .iter()
                .filter_map(|row| {
                    row.first()
                        .and_then(|value| value.as_str())
                        .map(|text| text.to_string())
                })
                .collect::<Vec<_>>();
            let available = names.iter().any(|name| name == "pldbgapi");
            Ok(DebugReport {
                available,
                message: if available {
                    "检测到 pldbgapi。调试页会另开一条目标连接执行调用，本连接只做控制：断点、单步、变量和栈。等待最多 20 秒。".into()
                } else if names.iter().any(|name| name == "plpgsql_check") {
                    "只有 plpgsql_check，可以做静态检查，不能单步。".into()
                } else {
                    "未安装 pldbgapi。CREATE EXTENSION 需要超级用户，本程序不会自动安装扩展。".into()
                },
                steps: vec![
                    "SELECT pldbg_create_listener() AS session;".into(),
                    "SELECT pldbg_set_global_breakpoint(session, (SELECT oid FROM pg_proc WHERE proname = '函数名'), -1, NULL);".into(),
                    "SELECT pldbg_wait_for_target(session); -- 会阻塞，请先 SET statement_timeout".into(),
                    "SELECT pldbg_step_over(session);".into(),
                    "SELECT pldbg_continue(session);".into(),
                    "SELECT pldbg_abort_target(session);".into(),
                ],
            })
        }
        Engine::Oracle => Ok(DebugReport {
            available: false,
            message: "没有 OCI，不能挂接 Oracle DBMS_DEBUG。sqlplus 只负责执行语句。".into(),
            steps: Vec::new(),
        }),
        Engine::Mysql | Engine::Mariadb => Ok(DebugReport {
            available: false,
            message: "MySQL 存储过程没有公开的单步调试协议。这里只生成过程 SQL，不假装能下断点。"
                .into(),
            steps: Vec::new(),
        }),
        other => Ok(DebugReport {
            available: false,
            message: format!("{} 没有实现过程调试器。", other.label()),
            steps: Vec::new(),
        }),
    }
}

pub fn redis_expire(
    state: &AppState,
    connection_id: &str,
    key: &str,
    seconds: i64,
) -> AppResult<String> {
    state.with_session(connection_id, |session| {
        if session.engine() != Engine::Redis {
            return Err(AppError::msg("只有 Redis 可以设置 TTL"));
        }
        let quoted = format!("'{}'", key.replace('\'', ""));
        session.execute("", &format!("EXPIRE {quoted} {seconds}"), None)?;
        Ok(format!(
            "已把 {key} 的过期时间设为 {seconds} 秒。0 表示立即过期。"
        ))
    })
}
