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

    /// Commit already validated DML on one database session. Drivers without a
    /// persistent session must override this rather than emulate BEGIN calls.
    fn execute_transaction(&mut self, database: &str, statements: &[String]) -> AppResult<()> {
        if statements.is_empty() {
            return Ok(());
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
        for (index, sql) in statements.iter().enumerate() {
            if let Err(err) = self.execute(database, sql, None) {
                let rollback = self.execute(database, "ROLLBACK", None);
                return Err(AppError::msg(match rollback {
                    Ok(_) => format!("第 {} 条语句失败，已回滚：{err}", index + 1),
                    Err(rollback_error) => format!("第 {} 条语句失败：{err}；回滚也失败：{rollback_error}。请检查连接和目标数据", index + 1),
                }));
            }
        }
        if let Err(err) = self.execute(database, "COMMIT", None) {
            let _ = self.execute(database, "ROLLBACK", None);
            return Err(err);
        }
        Ok(())
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
