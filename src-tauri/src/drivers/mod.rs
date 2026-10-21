use crate::error::{AppError, AppResult};
use crate::models::{
    ColumnMeta, Engine, FilterSpec, QueryOutcome, RowChange, SessionConfig, SortSpec,
};

pub mod dameng;
pub mod mongo;
pub mod mssql;
pub mod mysql;
pub mod oracle;
pub mod postgres;
pub mod redis;
pub mod runtime;
pub mod snowflake;
pub mod sqlite;

#[cfg(test)]
mod control_tests;
#[cfg(test)]
mod document_tests;

pub trait DbSession: Send {
    fn engine(&self) -> Engine;
    fn version(&mut self) -> AppResult<String>;
    fn list_databases(&mut self) -> AppResult<Vec<String>>;
    fn list_schemas(&mut self, database: &str) -> AppResult<Vec<String>>;
    fn list_tables(&mut self, database: &str, schema: &str) -> AppResult<Vec<String>>;
    fn list_views(&mut self, database: &str, schema: &str) -> AppResult<Vec<String>>;
    fn list_columns(
        &mut self,
        database: &str,
        schema: &str,
        object_name: &str,
    ) -> AppResult<Vec<ColumnMeta>>;
    fn relation_kind(
        &mut self,
        database: &str,
        schema: &str,
        object_name: &str,
    ) -> AppResult<Option<String>>;
    fn execute(
        &mut self,
        database: &str,
        sql: &str,
        max_rows: Option<usize>,
    ) -> AppResult<QueryOutcome>;

    /// A cancellation channel independent of the mutex protecting this session.
    /// The caller must retire the handle before allowing another query to start.
    fn cancel_handle(
        &mut self,
        _database: &str,
    ) -> AppResult<Option<crate::operations::CancelHandle>> {
        Ok(None)
    }

    /// Used only by a dedicated scan session, never the user's query session.
    fn begin_read_snapshot(&mut self, _database: &str) -> AppResult<()> {
        Err(AppError::msg(
            "当前驱动尚不支持一致性快照，不能执行无上限流式同步",
        ))
    }

    fn end_read_snapshot(&mut self, _database: &str) -> AppResult<()> {
        Err(AppError::msg("当前驱动没有可结束的一致性快照"))
    }

    fn validate_snapshot_relation(
        &mut self,
        _database: &str,
        _schema: &str,
        _table: &str,
    ) -> AppResult<()> {
        Ok(())
    }

    fn execute_affected(&mut self, database: &str, sql: &str) -> AppResult<u64> {
        self.execute(database, sql, None)
            .map(|outcome| outcome.rows_affected)
    }

    /// Commit already validated DML on one database session. Drivers without a
    /// persistent session must override this rather than emulate BEGIN calls.
    #[cfg(test)]
    fn execute_transaction(&mut self, database: &str, statements: &[String]) -> AppResult<()> {
        let checked = statements
            .iter()
            .cloned()
            .map(|sql| (sql, None))
            .collect::<Vec<_>>();
        self.execute_checked_transaction(database, &checked)
            .map(|_| ())
    }

    /// Check affected rows before COMMIT so a stale edit rolls back the batch.
    fn execute_transaction_with_operation(
        &mut self,
        database: &str,
        statements: &[String],
        operation: &crate::operations::OperationContext,
    ) -> AppResult<()> {
        let checked = statements
            .iter()
            .cloned()
            .map(|sql| (sql, None))
            .collect::<Vec<_>>();
        self.execute_checked_transaction_with_operation(database, &checked, operation)
            .map(|_| ())
    }

    fn execute_checked_transaction(
        &mut self,
        database: &str,
        statements: &[(String, Option<u64>)],
    ) -> AppResult<usize> {
        self.execute_checked_transaction_with_operation(
            database,
            statements,
            &crate::operations::OperationContext::detached(),
        )
    }

    fn execute_checked_transaction_with_operation(
        &mut self,
        database: &str,
        statements: &[(String, Option<u64>)],
        operation: &crate::operations::OperationContext,
    ) -> AppResult<usize> {
        operation.checkpoint()?;
        if self.engine() == Engine::Oracle {
            return self.execute_checked_transaction(database, statements);
        }
        if statements.is_empty() {
            return Ok(0);
        }
        if self.is_document_store() || matches!(self.engine(), Engine::Oracle | Engine::Snowflake) {
            return Err(AppError::msg("当前驱动不能保证同一会话事务，已拒绝网格和 CSV 批量修改；请使用该数据库的原生客户端"));
        }
        let begin = if self.engine() == Engine::Mssql {
            "BEGIN TRANSACTION"
        } else {
            "BEGIN"
        };
        self.execute(database, begin, None)?;
        for (index, (sql, expected)) in statements.iter().enumerate() {
            operation.report(
                "写入事务",
                index as u64,
                Some(statements.len() as u64),
                "当前事务尚未提交；取消将等待当前语句结束并回滚本批",
            );
            let result = operation
                .checkpoint()
                .and_then(|_| self.execute_affected(database, sql))
                .and_then(|affected| {
                    if expected.is_some_and(|count| count != affected) {
                        Err(AppError::msg(format!(
                            "数据已被修改或删除，预期影响 {} 行，实际 {} 行；请重新加载后检查更改",
                            expected.unwrap_or(0),
                            affected
                        )))
                    } else {
                        Ok(())
                    }
                });
            if let Err(err) = result {
                let rollback = self.execute(database, "ROLLBACK", None);
                if matches!(err, AppError::Cancelled { .. }) && rollback.is_ok() {
                    return Err(err);
                }
                return Err(AppError::msg(match rollback {
                    Ok(_) => format!("第 {} 条语句失败，已回滚：{err}", index + 1),
                    Err(rollback_error) => format!("第 {} 条语句失败：{err}；回滚也失败：{rollback_error}。请检查连接和目标数据", index + 1),
                }));
            }
        }
        if let Err(cancelled) = operation.checkpoint() {
            self.execute(database, "ROLLBACK", None)
                .map_err(|error| AppError::msg(format!("取消时回滚失败，请核对数据库：{error}")))?;
            return Err(cancelled);
        }
        if let Err(err) = self.execute(database, "COMMIT", None) {
            let _ = self.execute(database, "ROLLBACK", None);
            return Err(err);
        }
        Ok(statements.len())
    }

    fn is_document_store(&self) -> bool {
        false
    }

    fn browse_page(
        &mut self,
        _database: &str,
        _schema: &str,
        _table: &str,
        _filters: &[FilterSpec],
        _sorts: &[SortSpec],
        _limit: u32,
        _offset: u32,
    ) -> AppResult<QueryOutcome> {
        Err(AppError::msg("这个引擎使用 SQL 读取数据"))
    }

    fn count_relation(&mut self, _database: &str, _schema: &str, _table: &str) -> AppResult<u64> {
        Err(AppError::msg("这个引擎使用 SQL 计数"))
    }

    fn count_filtered_relation(
        &mut self,
        database: &str,
        schema: &str,
        table: &str,
        filters: &[FilterSpec],
    ) -> AppResult<u64> {
        if !filters.is_empty() {
            return Err(AppError::msg("当前引擎不支持网格条件筛选"));
        }
        self.count_relation(database, schema, table)
    }

    fn apply_mutation(
        &mut self,
        _database: &str,
        _schema: &str,
        _table: &str,
        _change: &RowChange,
    ) -> AppResult<()> {
        Err(AppError::msg("这个引擎使用 SQL 提交更改"))
    }
}

pub fn open_session(config: &SessionConfig) -> AppResult<Box<dyn DbSession>> {
    match config.engine {
        Engine::Mysql | Engine::Mariadb => Ok(Box::new(mysql::connect(config)?)),
        Engine::Postgres => Ok(Box::new(postgres::connect(config)?)),
        Engine::Sqlite => Ok(Box::new(sqlite::connect(config)?)),
        Engine::Mongodb => Ok(Box::new(mongo::connect(config)?)),
        Engine::Redis => Ok(Box::new(redis::connect(config)?)),
        Engine::Mssql => Ok(Box::new(mssql::connect(config)?)),
        Engine::Oracle => Ok(Box::new(oracle::connect(config)?)),
        Engine::Snowflake => Ok(Box::new(snowflake::connect(config)?)),
        Engine::Dameng => Ok(Box::new(dameng::connect(config)?)),
    }
}
