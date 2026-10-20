use std::time::Duration;

use mysql::prelude::Queryable;
use mysql::{OptsBuilder, SslOpts, Value};
use serde_json::Value as JsonValue;

use super::DbSession;
use crate::error::{AppError, AppResult};
use crate::models::{ColumnMeta, Engine, QueryOutcome, SessionConfig};
use crate::sqlutil::{json_blob, json_f64, json_i64, json_u64, sql_literal};

pub struct MysqlSession {
    conn: mysql::Conn,
    engine: Engine,
    current_db: String,
}

pub fn connect(config: &SessionConfig) -> AppResult<MysqlSession> {
    if config.use_ssl && config.tls_server_name.is_some() && !config.ssl_insecure {
        return Err(AppError::msg("当前 MySQL / MariaDB 驱动无法在隧道连接中保留 TLS 证书主机名，已拒绝连接；请使用直接 TLS 连接。"));
    }
    let mut opts = OptsBuilder::new()
        .ip_or_hostname(Some(config.host.trim().to_string()))
        .tcp_port(if config.port == 0 {
            config.engine.default_port()
        } else {
            config.port
        })
        .user(Some(config.username.clone()))
        .pass(Some(config.password.clone()))
        .prefer_socket(false)
        .tcp_connect_timeout(Some(Duration::from_secs(8)))
        .read_timeout(Some(Duration::from_secs(60)))
        .write_timeout(Some(Duration::from_secs(60)));
    if !config.database.trim().is_empty() {
        opts = opts.db_name(Some(config.database.trim().to_string()));
    }
    let auth = config.auth_method.to_ascii_lowercase();
    if matches!(auth.as_str(), "cleartext" | "ldap" | "pam") {
        opts = opts.enable_cleartext_plugin(true);
    }
    if config.use_ssl {
        let mut ssl = SslOpts::default();
        if config.ssl_insecure {
            ssl = ssl
                .with_danger_accept_invalid_certs(true)
                .with_danger_skip_domain_validation(true);
        }
        opts = opts.ssl_opts(Some(ssl));
    }
    let conn = mysql::Conn::new(opts).map_err(|err| AppError::msg(err.to_string()))?;
    Ok(MysqlSession {
        conn,
        engine: config.engine,
        current_db: config.database.trim().to_string(),
    })
}

impl DbSession for MysqlSession {
    fn engine(&self) -> Engine {
        self.engine
    }

    fn version(&mut self) -> AppResult<String> {
        let outcome = self.execute("", "SELECT VERSION()", Some(1))?;
        first_text(&outcome).ok_or_else(|| AppError::msg("无法读取服务器版本"))
    }

    fn list_databases(&mut self) -> AppResult<Vec<String>> {
        self.one_column("SHOW DATABASES")
    }

    fn list_schemas(&mut self, database: &str) -> AppResult<Vec<String>> {
        if database.is_empty() {
            Ok(Vec::new())
        } else {
            Ok(vec![database.to_string()])
        }
    }

    fn list_tables(&mut self, database: &str, _schema: &str) -> AppResult<Vec<String>> {
        let db = sql_literal(self.engine, &JsonValue::String(database.to_string()))?;
        self.one_column(&format!(
            "SELECT TABLE_NAME FROM information_schema.TABLES WHERE TABLE_SCHEMA = {db} AND TABLE_TYPE = 'BASE TABLE' ORDER BY TABLE_NAME"
        ))
    }

    fn list_views(&mut self, database: &str, _schema: &str) -> AppResult<Vec<String>> {
        let db = sql_literal(self.engine, &JsonValue::String(database.to_string()))?;
        self.one_column(&format!(
            "SELECT TABLE_NAME FROM information_schema.TABLES WHERE TABLE_SCHEMA = {db} AND TABLE_TYPE = 'VIEW' ORDER BY TABLE_NAME"
        ))
    }

    fn list_columns(
        &mut self,
        database: &str,
        _schema: &str,
        object_name: &str,
    ) -> AppResult<Vec<ColumnMeta>> {
        let db = sql_literal(self.engine, &JsonValue::String(database.to_string()))?;
        let table = sql_literal(self.engine, &JsonValue::String(object_name.to_string()))?;
        let sql = format!(
            "SELECT COLUMN_NAME, DATA_TYPE, IS_NULLABLE, COLUMN_DEFAULT, COLUMN_KEY, EXTRA \
             FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = {db} AND TABLE_NAME = {table} \
             ORDER BY ORDINAL_POSITION"
        );
        let rows = self.query_table(&sql)?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let extra = row.get(5).cloned().unwrap_or_default();
                ColumnMeta {
                    name: row.first().cloned().unwrap_or_default(),
                    data_type: row.get(1).cloned().unwrap_or_default(),
                    nullable: row.get(2).map(|value| value.eq_ignore_ascii_case("YES")).unwrap_or(true),
                    primary_key: row.get(4).map(|value| value == "PRI").unwrap_or(false),
                    auto_increment: extra.to_ascii_lowercase().contains("auto_increment"),
                    default_value: row.get(3).cloned().filter(|value| !value.is_empty()),
                }
            })
            .collect())
    }

    fn relation_kind(
        &mut self,
        database: &str,
        _schema: &str,
        object_name: &str,
    ) -> AppResult<Option<String>> {
        let db = sql_literal(self.engine, &JsonValue::String(database.to_string()))?;
        let table = sql_literal(self.engine, &JsonValue::String(object_name.to_string()))?;
        let rows = self.one_column(&format!(
            "SELECT TABLE_TYPE FROM information_schema.TABLES WHERE TABLE_SCHEMA = {db} AND TABLE_NAME = {table}"
        ))?;
        Ok(rows.into_iter().next().map(|kind| {
            if kind.eq_ignore_ascii_case("VIEW") {
                "view".to_string()
            } else {
                "table".to_string()
            }
        }))
    }

    fn execute(
        &mut self,
        database: &str,
        sql: &str,
        max_rows: Option<usize>,
    ) -> AppResult<QueryOutcome> {
        let sql = sql.trim();
        if sql.is_empty() {
            return Ok(QueryOutcome::empty());
        }
        self.activate(database)?;
        let mut result = self
            .conn
            .query_iter(sql)
            .map_err(|err| AppError::msg(err.to_string()))?;
        let columns: Vec<ColumnMeta> = result
            .columns()
            .as_ref()
            .iter()
            .map(|column| {
                let mut meta = ColumnMeta::named(column.name_str().to_string());
                meta.data_type = format!("{:?}", column.column_type());
                meta
            })
            .collect();
        if columns.is_empty() {
            while result
                .next()
                .transpose()
                .map_err(|err| AppError::msg(err.to_string()))?
                .is_some()
            {}
            let mut outcome = QueryOutcome::empty();
            outcome.rows_affected = result.affected_rows();
            return Ok(outcome);
        }
        let mut rows = Vec::new();
        let mut truncated = false;
        while let Some(row) = result
            .next()
            .transpose()
            .map_err(|err| AppError::msg(err.to_string()))?
        {
            if max_rows.is_some_and(|limit| rows.len() >= limit) {
                truncated = true;
                break;
            }
            let mut cells = Vec::with_capacity(columns.len());
            for index in 0..columns.len() {
                let value = row.get(index).unwrap_or(Value::NULL);
                cells.push(mysql_to_json(value));
            }
            rows.push(cells);
        }
        Ok(QueryOutcome {
            columns,
            rows,
            rows_affected: 0,
            truncated,
        })
    }
}

impl MysqlSession {
    fn activate(&mut self, database: &str) -> AppResult<()> {
        let database = database.trim();
        if database.is_empty() || database == self.current_db {
            return Ok(());
        }
        let ident = crate::sqlutil::quote_ident(self.engine, database)?;
        self.conn
            .query_drop(format!("USE {ident}"))
            .map_err(|err| AppError::msg(err.to_string()))?;
        self.current_db = database.to_string();
        Ok(())
    }

    fn one_column(&mut self, sql: &str) -> AppResult<Vec<String>> {
        Ok(self
            .query_table(sql)?
            .into_iter()
            .filter_map(|row| row.into_iter().next())
            .collect())
    }

    fn query_table(&mut self, sql: &str) -> AppResult<Vec<Vec<String>>> {
        let outcome = self.execute("", sql, None)?;
        Ok(outcome
            .rows
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|value| match value {
                        JsonValue::Null => String::new(),
                        JsonValue::String(text) => text,
                        other => other.to_string(),
                    })
                    .collect()
            })
            .collect())
    }
}

fn mysql_to_json(value: Value) -> JsonValue {
    match value {
        Value::NULL => JsonValue::Null,
        Value::Bytes(bytes) => match String::from_utf8(bytes) {
            Ok(text) => JsonValue::String(text),
            Err(err) => json_blob(err.as_bytes()),
        },
        Value::Int(number) => json_i64(number),
        Value::UInt(number) => json_u64(number),
        Value::Float(number) => json_f64(number as f64),
        Value::Double(number) => json_f64(number),
        Value::Date(year, month, day, hour, minute, second, _micros) => {
            if hour == 0 && minute == 0 && second == 0 {
                JsonValue::String(format!("{year:04}-{month:02}-{day:02}"))
            } else {
                JsonValue::String(format!(
                    "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}"
                ))
            }
        }
        Value::Time(negative, days, hour, minute, second, _micros) => {
            let sign = if negative { "-" } else { "" };
            if days > 0 {
                JsonValue::String(format!(
                    "{sign}{days}d {hour:02}:{minute:02}:{second:02}"
                ))
            } else {
                JsonValue::String(format!("{sign}{hour:02}:{minute:02}:{second:02}"))
            }
        }
    }
}

fn first_text(outcome: &QueryOutcome) -> Option<String> {
    outcome.rows.first()?.first().map(|value| match value {
        JsonValue::String(text) => text.clone(),
        JsonValue::Null => String::new(),
        other => other.to_string(),
    })
}
