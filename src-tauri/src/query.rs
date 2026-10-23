use std::time::Instant;

use crate::error::{AppError, AppResult};
use crate::models::{ExecuteSqlRequest, ExecuteSqlResponse, QueryOutcome, StatementResult};
use crate::operations::OperationContext;
use crate::sqlutil::{is_pageable, paginate_query_sql, split_sql};
use crate::state::AppState;

pub fn execute_sql(state: &AppState, request: ExecuteSqlRequest) -> AppResult<ExecuteSqlResponse> {
    execute_sql_with_operation(state, request, &OperationContext::detached())
}

pub fn execute_sql_with_operation(
    state: &AppState,
    request: ExecuteSqlRequest,
    operation: &OperationContext,
) -> AppResult<ExecuteSqlResponse> {
    operation.checkpoint()?;
    let page = request.page.max(1);
    let page_size = request.page_size.clamp(1, 1000);
    let statements = split_sql(&request.sql);
    if statements.is_empty() {
        return Err(AppError::msg("没有可执行的 SQL"));
    }
    if let Some(index) = request.only_statement {
        if index >= statements.len() {
            return Err(AppError::msg("语句序号超出范围"));
        }
    }

    state.with_query_session(
        &request.connection_id,
        &request.database,
        request.session_id.as_deref(),
        operation,
        |session| {
            if !session.engine().capabilities().persistent_console
                && statements.iter().any(|sql| {
                    matches!(
                        crate::sqlutil::strip_leading_comments(sql)
                            .trim()
                            .trim_end_matches(';')
                            .trim()
                            .to_ascii_uppercase()
                            .as_str(),
                        "BEGIN" | "BEGIN TRANSACTION" | "START TRANSACTION" | "COMMIT" | "ROLLBACK"
                    )
                })
            {
                return Err(AppError::msg(
                    "当前驱动不支持跨语句的会话事务，请使用该引擎支持的原生事务操作",
                ));
            }
            let mut results = Vec::new();
            let mut stopped = false;
            for (index, sql) in statements.iter().enumerate() {
                if request.only_statement.is_some_and(|only| only != index) {
                    continue;
                }
                operation.checkpoint()?;
                let total = if request.only_statement.is_some() {
                    1
                } else {
                    statements.len()
                };
                operation.report(
                    "执行 SQL",
                    results.len() as u64,
                    Some(total as u64),
                    "取消会停止当前支持中断的语句及后续语句，已完成的写入不会自动撤销",
                );
                let started = Instant::now();
                match run_one(session, &request.database, sql, page, page_size, operation) {
                    Ok(mut result) => {
                        result.index = index;
                        result.sql = sql.clone();
                        result.elapsed_ms = started.elapsed().as_millis() as u64;
                        results.push(result);
                        operation.report(
                            "执行 SQL",
                            results.len() as u64,
                            Some(total as u64),
                            "当前语句已完成；原始 SQL 可能处于显式事务，影响行数不等于已提交数量",
                        );
                        operation.checkpoint()?;
                    }
                    Err(err) => {
                        if matches!(err, AppError::Cancelled { .. }) {
                            return Err(err);
                        }
                        operation.report_failure(err.to_string());
                        results.push(StatementResult {
                            index,
                            sql: sql.clone(),
                            ok: false,
                            error: Some(err.to_string()),
                            columns: Vec::new(),
                            rows: Vec::new(),
                            rows_affected: 0,
                            total_rows: None,
                            page,
                            page_size,
                            pageable: is_pageable(sql),
                            elapsed_ms: started.elapsed().as_millis() as u64,
                            message: err.to_string(),
                            truncated: false,
                            has_more: false,
                        });
                        stopped = true;
                        break;
                    }
                }
            }
            Ok(ExecuteSqlResponse {
                statements: results,
                stopped,
            })
        },
    )
}

fn run_one(
    session: &mut dyn crate::drivers::DbSession,
    database: &str,
    sql: &str,
    page: u32,
    page_size: u32,
    operation: &OperationContext,
) -> AppResult<StatementResult> {
    let offset = (page - 1)
        .checked_mul(page_size)
        .ok_or_else(|| AppError::msg("分页位置超出范围"))?;
    let paged = paginate_query_sql(session.engine(), sql, page_size + 1, offset);
    if let Some(paged) = paged {
        match execute_one(
            session,
            database,
            &paged,
            Some(page_size as usize + 1),
            operation,
        ) {
            Ok(mut outcome) => {
                if outcome.truncated && outcome.rows.len() <= page_size as usize {
                    return Err(AppError::msg("当前页结果超过读取预算，请减少每页行数或所选字段；分页已停止以避免跳过未显示的记录"));
                }
                let has_more = outcome.truncated || outcome.rows.len() > page_size as usize;
                outcome.rows.truncate(page_size as usize);
                // Never execute arbitrary user SQL a second time to count it:
                // SELECT and materialized CTEs can invoke stateful functions.
                let total = if !has_more {
                    Some(u64::from(offset) + outcome.rows.len() as u64)
                } else {
                    None
                };
                let mut result = from_outcome(outcome, page, page_size, true, total);
                result.has_more = has_more;
                return Ok(result);
            }
            Err(err) => return Err(err),
        }
    }
    if page > 1 {
        return Err(AppError::msg("这条语句按原文执行，不支持自动分页"));
    }
    let outcome = execute_one(session, database, sql, Some(5_000), operation)?;
    Ok(from_outcome(outcome, page, page_size, false, None))
}

fn execute_one(
    session: &mut dyn crate::drivers::DbSession,
    database: &str,
    sql: &str,
    limit: Option<usize>,
    operation: &OperationContext,
) -> AppResult<QueryOutcome> {
    // Never interrupt a transaction completion: its result determines commit ownership.
    let completion = matches!(
        crate::sqlutil::strip_leading_comments(sql)
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_end_matches(';')
            .to_ascii_uppercase()
            .as_str(),
        "COMMIT" | "ROLLBACK" | "END"
    );
    let interrupt = operation.bind_interrupt(if completion {
        None
    } else {
        session.cancel_handle(database)?
    })?;
    operation.checkpoint()?;
    let result = session.execute(database, sql, limit);
    drop(interrupt);
    if result.is_err() && operation.cancel_requested() {
        return Err(operation.cancel_error());
    }
    result
}

fn from_outcome(
    outcome: QueryOutcome,
    page: u32,
    page_size: u32,
    pageable: bool,
    total_rows: Option<u64>,
) -> StatementResult {
    let message = if outcome.columns.is_empty() {
        format!("影响 {} 行", outcome.rows_affected)
    } else if let Some(total) = total_rows {
        format!("本页 {} 行，共 {total} 行", outcome.rows.len())
    } else {
        format!("返回 {} 行", outcome.rows.len())
    };
    StatementResult {
        index: 0,
        sql: String::new(),
        ok: true,
        error: None,
        columns: outcome.columns,
        rows: outcome.rows,
        rows_affected: outcome.rows_affected,
        total_rows,
        page,
        page_size,
        pageable,
        elapsed_ms: 0,
        message,
        truncated: outcome.truncated,
        has_more: false,
    }
}

#[cfg(test)]
mod tests;
