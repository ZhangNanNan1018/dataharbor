use serde_json::Value as JsonValue;

use super::DbSession;
use crate::error::{AppError, AppResult};
use crate::models::{ColumnMeta, Engine, QueryOutcome, SessionConfig};
use crate::sqlutil::{json_f64, json_i64, sql_literal};

pub struct DamengSession {
    client: dameng::Client,
    username: String,
    database: String,
}

pub fn connect(config: &SessionConfig) -> AppResult<DamengSession> {
    if config.use_ssl && config.tls_server_name.is_some() {
        return Err(AppError::msg("当前达梦驱动不能为隧道单独指定 TLS 证书主机名，已拒绝 SSL 隧道连接；请使用直接 TLS 连接。"));
    }
    let host = config.host.trim();
    if host.is_empty() {
        return Err(AppError::msg("达梦需要主机地址"));
    }
    let port = if config.port == 0 { 5236 } else { config.port };
    let mut client = dameng::Client::new(host, port);
    let connected = if config.use_ssl {
        client.connect_ssl(&config.username, &config.password)
    } else {
        client.connect(&config.username, &config.password)
    };
    connected.map_err(|err| {
        AppError::msg(format!(
            "达梦连接失败: {err}。这是 dameng crate 的 DM8 线协议（默认端口 5236），不是 ODBC。TLS 使用系统信任库，不能跳过证书校验。"
        ))
    })?;
    Ok(DamengSession {
        client,
        username: config.username.clone(),
        database: config.database.trim().to_string(),
    })
}

impl DamengSession {
    fn schema_of(&self, schema: &str) -> String {
        let schema = schema.trim();
        if !schema.is_empty() {
            return schema.to_string();
        }
        if !self.database.is_empty() {
            return self.database.clone();
        }
        self.username.to_uppercase()
    }

    fn query(&mut self, sql: &str) -> AppResult<QueryOutcome> {
        let result = self
            .client
            .query(sql)
            .map_err(|err| AppError::msg(err.to_string()))?;
        let columns = result
            .columns
            .iter()
            .map(|column| ColumnMeta {
                name: column.name.clone(),
                data_type: column.type_name.clone(),
                nullable: column.nullable,
                primary_key: false,
                auto_increment: false,
                default_value: None,
            })
            .collect::<Vec<_>>();
        let mut rows = Vec::new();
        for row in result.iter() {
            let mut cells = Vec::with_capacity(columns.len());
            for index in 0..columns.len() {
                cells.push(cell_value(&row, index, &columns[index].data_type));
            }
            rows.push(cells);
        }
        Ok(QueryOutcome {
            columns,
            rows,
            rows_affected: 0,
            truncated: false,
        })
    }

    fn one_column(&mut self, sql: &str) -> AppResult<Vec<String>> {
        let outcome = self.query(sql)?;
        Ok(outcome
            .rows
            .into_iter()
            .filter_map(|row| row.into_iter().next())
            .map(|value| match value {
                JsonValue::String(text) => text,
                JsonValue::Null => String::new(),
                other => other.to_string(),
            })
            .collect())
    }
}

impl DbSession for DamengSession {
    fn engine(&self) -> Engine {
        Engine::Dameng
    }

    fn version(&mut self) -> AppResult<String> {
        let values = self.one_column("SELECT BANNER FROM V$VERSION")?;
        let text = values.into_iter().filter(|item| !item.is_empty()).collect::<Vec<_>>().join(" · ");
        if text.is_empty() {
            Err(AppError::msg("无法读取达梦版本"))
        } else {
            Ok(text)
        }
    }

    fn list_databases(&mut self) -> AppResult<Vec<String>> {
        match self.one_column("SELECT NAME FROM V$INSTANCE") {
            Ok(names) if !names.is_empty() => Ok(names),
            _ => Ok(vec!["DAMENG".into()]),
        }
    }

    fn list_schemas(&mut self, _database: &str) -> AppResult<Vec<String>> {
        self.one_column("SELECT USERNAME FROM ALL_USERS ORDER BY USERNAME")
    }

    fn list_tables(&mut self, _database: &str, schema: &str) -> AppResult<Vec<String>> {
        let owner = sql_literal(Engine::Dameng, &JsonValue::String(self.schema_of(schema)))?;
        self.one_column(&format!(
            "SELECT TABLE_NAME FROM ALL_TABLES WHERE OWNER = {owner} ORDER BY TABLE_NAME"
        ))
    }

    fn list_views(&mut self, _database: &str, schema: &str) -> AppResult<Vec<String>> {
        let owner = sql_literal(Engine::Dameng, &JsonValue::String(self.schema_of(schema)))?;
        self.one_column(&format!(
            "SELECT VIEW_NAME FROM ALL_VIEWS WHERE OWNER = {owner} ORDER BY VIEW_NAME"
        ))
    }

    fn list_columns(&mut self, _database: &str, schema: &str, object_name: &str) -> AppResult<Vec<ColumnMeta>> {
        let owner = sql_literal(Engine::Dameng, &JsonValue::String(self.schema_of(schema)))?;
        let table = sql_literal(Engine::Dameng, &JsonValue::String(object_name.to_string()))?;
        let outcome = self.query(&format!(
            "SELECT COLUMN_NAME, DATA_TYPE, NULLABLE, DATA_DEFAULT FROM ALL_TAB_COLUMNS WHERE OWNER = {owner} AND TABLE_NAME = {table} ORDER BY COLUMN_ID"
        ))?;
        let pk = self
            .one_column(&format!(
                "SELECT cc.COLUMN_NAME FROM ALL_CONSTRAINTS c JOIN ALL_CONS_COLUMNS cc ON c.OWNER = cc.OWNER AND c.CONSTRAINT_NAME = cc.CONSTRAINT_NAME WHERE c.CONSTRAINT_TYPE = 'P' AND c.OWNER = {owner} AND c.TABLE_NAME = {table}"
            ))
            .unwrap_or_default();
        Ok(outcome
            .rows
            .into_iter()
            .map(|row| {
                let text = |index: usize| match row.get(index) {
                    Some(JsonValue::String(value)) => value.clone(),
                    Some(JsonValue::Null) | None => String::new(),
                    Some(other) => other.to_string(),
                };
                let name = text(0);
                let default_value = text(3);
                ColumnMeta {
                    primary_key: pk.iter().any(|item| item.eq_ignore_ascii_case(&name)),
                    auto_increment: default_value.to_ascii_uppercase().contains("IDENTITY"),
                    name,
                    data_type: text(1),
                    nullable: text(2).eq_ignore_ascii_case("Y"),
                    default_value: if default_value.is_empty() { None } else { Some(default_value) },
                }
            })
            .collect())
    }

    fn relation_kind(&mut self, database: &str, schema: &str, object_name: &str) -> AppResult<Option<String>> {
        let tables = self.list_tables(database, schema)?;
        if tables.iter().any(|item| item.eq_ignore_ascii_case(object_name)) {
            return Ok(Some("table".into()));
        }
        let views = self.list_views(database, schema)?;
        if views.iter().any(|item| item.eq_ignore_ascii_case(object_name)) {
            Ok(Some("view".into()))
        } else {
            Ok(None)
        }
    }

    fn execute(&mut self, _database: &str, sql: &str, max_rows: Option<usize>) -> AppResult<QueryOutcome> {
        let sql = sql.trim();
        if sql.is_empty() {
            return Ok(QueryOutcome::empty());
        }
        if looks_like_query(sql) {
            let mut outcome = self.query(sql)?;
            if max_rows.is_some_and(|limit| outcome.rows.len() > limit) {
                outcome.rows.truncate(max_rows.unwrap_or(0));
                outcome.truncated = true;
            }
            return Ok(outcome);
        }
        let affected = self
            .client
            .execute(sql)
            .map_err(|err| AppError::msg(err.to_string()))?;
        Ok(QueryOutcome {
            columns: Vec::new(),
            rows: Vec::new(),
            rows_affected: affected,
            truncated: false,
        })
    }
}

fn looks_like_query(sql: &str) -> bool {
    let head = sql
        .trim_start()
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_end_matches(';')
        .to_ascii_uppercase();
    matches!(head.as_str(), "SELECT" | "WITH" | "EXPLAIN" | "SHOW" | "DESC" | "DESCRIBE")
}

fn cell_value(row: &dameng::QueryRowRef<'_>, index: usize, type_name: &str) -> JsonValue {
    if row.is_null(index) {
        return JsonValue::Null;
    }
    let kind = type_name.to_ascii_uppercase();
    if kind.contains("CHAR") || kind.contains("TEXT") || kind.contains("CLOB") {
        return match row.get_str(index) {
            Ok(text) => JsonValue::String(text.to_string()),
            Err(_) => lossy(row, index),
        };
    }
    if kind.contains("TIMESTAMP") || kind == "DATE" || kind.contains("TIME") || kind.contains("DATETIME") {
        return match row.get_timestamp(index) {
            Ok(text) => JsonValue::String(text),
            Err(_) => lossy(row, index),
        };
    }
    if kind.contains("INT") || kind == "BIT" {
        return match row.get_i64(index) {
            Ok(value) => json_i64(value),
            Err(_) => match row.get_i32(index) {
                Ok(value) => JsonValue::Number(value.into()),
                Err(_) => lossy(row, index),
            },
        };
    }
    if kind.contains("FLOAT") || kind.contains("DOUBLE") || kind.contains("REAL") || kind.contains("DEC") || kind.contains("NUM") {
        return match row.get_f64(index) {
            Ok(value) => json_f64(value),
            Err(_) => lossy(row, index),
        };
    }
    if let Ok(text) = row.get_str(index) {
        return JsonValue::String(text.to_string());
    }
    lossy(row, index)
}

fn lossy(row: &dameng::QueryRowRef<'_>, index: usize) -> JsonValue {
    match row.row.values.get(index) {
        Some(Some(bytes)) => JsonValue::String(String::from_utf8_lossy(bytes).into_owned()),
        _ => JsonValue::Null,
    }
}
