//! Read-only scans keep one connection, metadata result and count for the whole table.
//! Only a complete primary-key ordering inside a supported snapshot may span pages.
use super::table_data::{BATCH, CEILING};
use crate::drivers::DbSession;
use crate::error::{AppError, AppResult};
use crate::models::{ColumnMeta, Engine};
use crate::operations::{InterruptGuard, OperationContext};
use crate::sqlutil::{
    json_count, paginate_sql, qualified_name, quote_ident, sql_literal, value_sql,
};
use crate::state::{AppState, IsolatedSession};
use serde_json::Value;

pub(super) struct ReadSession {
    pub session: IsolatedSession,
    pub snapshot: bool,
    database: String,
    interrupt: Option<InterruptGuard>,
}

impl ReadSession {
    pub fn open(
        state: &AppState,
        id: &str,
        database: &str,
        operation: &OperationContext,
    ) -> AppResult<Self> {
        operation.checkpoint()?;
        let mut session = state.open_isolated_session(id)?;
        let interrupt = operation.bind_interrupt(session.cancel_handle(database)?)?;
        let snapshot = matches!(
            session.engine(),
            Engine::Sqlite | Engine::Postgres | Engine::Mysql | Engine::Mariadb
        );
        if snapshot {
            let result = session.begin_read_snapshot(database);
            operation.checkpoint()?;
            result?;
        }
        Ok(Self {
            session,
            snapshot,
            database: database.into(),
            interrupt: Some(interrupt),
        })
    }

    pub fn finish(mut self) -> AppResult<()> {
        // Retire cancellation before ROLLBACK, including its background retries.
        self.interrupt.take();
        if self.snapshot {
            self.snapshot = false;
            self.session.end_read_snapshot(&self.database)?;
        }
        Ok(())
    }
}

impl Drop for ReadSession {
    fn drop(&mut self) {
        self.interrupt.take();
        if self.snapshot {
            let _ = self.session.end_read_snapshot(&self.database);
        }
    }
}

pub(super) struct TableReader<'a> {
    session: &'a mut dyn DbSession,
    database: &'a str,
    schema: &'a str,
    table: &'a str,
    name: String,
    pub columns: Vec<ColumnMeta>,
    pub total: u64,
    pub loaded: u64,
    pub streaming: bool,
    cap: u64,
    keys: Vec<usize>,
    last: Option<Vec<Value>>,
    done: bool,
    driver_truncated: bool,
}

impl<'a> TableReader<'a> {
    pub fn new(
        session: &'a mut dyn DbSession,
        snapshot: bool,
        database: &'a str,
        schema: &'a str,
        table: &'a str,
        ceiling: Option<u32>,
        operation: &OperationContext,
    ) -> AppResult<Self> {
        operation.checkpoint()?;
        let columns = session.list_columns(database, schema, table);
        operation.checkpoint()?;
        let columns = columns?;
        let document = session.is_document_store();
        if columns.is_empty() && !document {
            return Err(AppError::msg("找不到表结构，已停止读取"));
        }
        let name = qualified_name(session.engine(), database, schema, table)?;
        let keys = columns
            .iter()
            .enumerate()
            .filter(|(_, c)| c.primary_key)
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        let (total, unsafe_keys) = if document {
            let total = session.count_relation(database, schema, table);
            operation.checkpoint()?;
            (total?, false)
        } else {
            // SQLite permits dynamic storage types, including NULL primary keys.
            // Its driver encodes large integers and blobs as strings, so their
            // storage class cannot be recovered from the displayed value alone.
            // Detect ambiguous cursor values in the same COUNT, before any page.
            let unsafe_count = if session.engine() == Engine::Sqlite && !keys.is_empty() {
                let predicate = keys
                    .iter()
                    .map(|&i| {
                        quote_ident(session.engine(), &columns[i].name)
                            .map(|name| format!(
                                "({name} IS NULL OR typeof({name}) = 'blob' \
                                 OR (typeof({name}) = 'integer' AND ({name} < -9007199254740991 OR {name} > 9007199254740991)) \
                                 OR (typeof({name}) = 'real' AND abs({name}) > 1.7976931348623157e308) \
                                 OR (typeof({name}) = 'text' AND instr({name}, char(0)) > 0))"
                            ))
                    })
                    .collect::<AppResult<Vec<_>>>()?
                    .join(" OR ");
                format!(", COUNT(CASE WHEN {predicate} THEN 1 END)")
            } else {
                String::new()
            };
            let outcome = session.execute(
                database,
                &format!("SELECT COUNT(*){unsafe_count} FROM {name}"),
                Some(1),
            );
            operation.checkpoint()?;
            let outcome = outcome?;
            let row = outcome
                .rows
                .first()
                .ok_or_else(|| AppError::msg("无法确认表的完整行数，已停止读取"))?;
            let total = row
                .first()
                .and_then(json_count)
                .ok_or_else(|| AppError::msg("无法确认表的完整行数，已停止读取"))?;
            let unsafe_values = if unsafe_count.is_empty() {
                false
            } else {
                row.get(1)
                    .and_then(json_count)
                    .ok_or_else(|| AppError::msg("无法确认主键能否安全续读，已停止读取"))?
                    > 0
            };
            (total, unsafe_values)
        };
        operation.checkpoint()?;
        // Failure to prove snapshot support uses one bounded read, never OFFSET
        // pages whose ordering could differ between executions.
        let stable_relation = snapshot
            && session
                .validate_snapshot_relation(database, schema, table)
                .is_ok();
        operation.checkpoint()?;
        let streaming = stable_relation && !keys.is_empty() && !unsafe_keys && !document;
        let cap = u64::from(ceiling.unwrap_or(if streaming { u32::MAX } else { CEILING }));
        // None means unbounded only for a supported keyset scan, not u32::MAX rows.
        let cap = if streaming && ceiling.is_none() {
            u64::MAX
        } else {
            cap
        };
        Ok(Self {
            session,
            database,
            schema,
            table,
            name,
            columns,
            total,
            loaded: 0,
            streaming,
            cap,
            keys,
            last: None,
            done: false,
            driver_truncated: false,
        })
    }

    pub fn truncated(&self) -> bool {
        self.driver_truncated || self.loaded < self.total
    }

    pub fn next(&mut self, operation: &OperationContext) -> AppResult<Option<Vec<Vec<Value>>>> {
        operation.checkpoint()?;
        if self.done || self.loaded >= self.cap || self.loaded == self.total && self.loaded > 0 {
            return Ok(None);
        }
        let limit = if self.streaming {
            u64::from(BATCH).min(self.cap - self.loaded)
        } else {
            self.cap
        } as u32;
        if limit == 0 {
            self.done = true;
            return Ok(None);
        }
        let outcome = if self.session.is_document_store() {
            self.session
                .browse_page(self.database, self.schema, self.table, &[], &[], limit, 0)
        } else {
            let sql = if self.streaming {
                self.keyset_sql(limit)?
            } else {
                let projection = self.projection()?;
                paginate_sql(
                    self.session.engine(),
                    &format!("SELECT {projection} FROM {}", self.name),
                    limit,
                    0,
                )
            };
            self.session
                .execute(self.database, &sql, Some(limit as usize))
        };
        operation.checkpoint()?;
        let outcome = outcome?;
        if self.session.is_document_store() && !outcome.columns.is_empty() {
            self.columns = outcome.columns;
        } else if !outcome.columns.is_empty()
            && outcome
                .columns
                .iter()
                .map(|c| &c.name)
                .ne(self.columns.iter().map(|c| &c.name))
        {
            return Err(AppError::msg("读取结果列顺序与表结构不一致，已停止读取"));
        }
        if outcome.rows.len() > limit as usize
            || outcome
                .rows
                .iter()
                .any(|row| row.len() != self.columns.len())
            || self.streaming && outcome.truncated
        {
            return Err(AppError::msg("读取被截断或列数不匹配，已停止读取"));
        }
        self.loaded += outcome.rows.len() as u64;
        self.driver_truncated |= outcome.truncated;
        if self.loaded > self.total {
            return Err(AppError::msg("读取行数超过初始总数，请停止写入源表后重试"));
        }
        if self.streaming {
            if let Some(row) = outcome.rows.last() {
                self.last = Some(self.keys.iter().map(|&i| row[i].clone()).collect());
            }
        }
        self.done = !self.streaming
            || outcome.rows.len() < limit as usize
            || self.loaded >= self.total
            || self.loaded >= self.cap;
        if self.done && self.loaded < self.total.min(self.cap) {
            return Err(AppError::msg("未完整读取预期数据，请停止写入源表后重试"));
        }
        operation.report(
            "读取并写出表数据",
            self.loaded,
            Some(self.total),
            self.table,
        );
        if outcome.rows.is_empty() {
            Ok(None)
        } else {
            Ok(Some(outcome.rows))
        }
    }

    fn projection(&self) -> AppResult<String> {
        Ok(self
            .columns
            .iter()
            .map(|c| quote_ident(self.session.engine(), &c.name))
            .collect::<AppResult<Vec<_>>>()?
            .join(", "))
    }

    fn keyset_sql(&self, limit: u32) -> AppResult<String> {
        let engine = self.session.engine();
        let names = self
            .keys
            .iter()
            .map(|&i| {
                let name = quote_ident(engine, &self.columns[i].name)?;
                // BINARY gives a unique ordering even when a SQLite PK index and
                // the declared column use different text collations. No cross-db
                // equality assumptions are needed for a read-only scan.
                Ok(if engine == Engine::Sqlite {
                    format!("{name} COLLATE BINARY")
                } else {
                    name
                })
            })
            .collect::<AppResult<Vec<_>>>()?;
        let predicate = if let Some(last) = &self.last {
            let values = self
                .keys
                .iter()
                .zip(last)
                .map(|(&i, value)| {
                    if value.is_null() {
                        return Err(AppError::msg("主键游标出现空值，已停止读取"));
                    }
                    if engine == Engine::Sqlite {
                        sql_literal(engine, value)
                    } else {
                        value_sql(engine, &self.columns[i].data_type, value)
                    }
                })
                .collect::<AppResult<Vec<_>>>()?;
            let alternatives = (0..self.keys.len())
                .map(|i| {
                    let mut parts = (0..i)
                        .map(|j| format!("{} = {}", names[j], values[j]))
                        .collect::<Vec<_>>();
                    parts.push(format!("{} > {}", names[i], values[i]));
                    format!("({})", parts.join(" AND "))
                })
                .collect::<Vec<_>>()
                .join(" OR ");
            format!(" WHERE {alternatives}")
        } else {
            String::new()
        };
        Ok(format!(
            "SELECT {} FROM {}{predicate} ORDER BY {} LIMIT {limit}",
            self.projection()?,
            self.name,
            names.join(", ")
        ))
    }
}

#[cfg(test)]
#[path = "read_batches_tests.rs"]
mod tests;
