use serde_json::{json, Value as JsonValue};

use super::DbSession;
use crate::error::{AppError, AppResult};
use crate::models::{ColumnMeta, Engine, QueryOutcome, SessionConfig};

pub struct SnowflakeSession {
    host: String,
    token: String,
    warehouse: String,
    database: String,
    schema: String,
}

pub fn connect(config: &SessionConfig) -> AppResult<SnowflakeSession> {
    if config.password.trim().is_empty() {
        return Err(AppError::msg("Snowflake 需要在密码栏填写访问令牌（PAT 或 OAuth token）"));
    }
    let host = snowflake_host(&config.host);
    let session = SnowflakeSession {
        host,
        token: config.password.clone(),
        warehouse: config.warehouse.trim().to_string(),
        database: config.database.trim().to_string(),
        schema: if config.schema_name.trim().is_empty() {
            "PUBLIC".into()
        } else {
            config.schema_name.trim().to_string()
        },
    };
    let mut session = session;
    let version = session.version()?;
    let _ = version;
    Ok(session)
}

fn snowflake_host(host: &str) -> String {
    let host = host.trim().trim_start_matches("https://").trim_end_matches('/');
    if host.ends_with("snowflakecomputing.com") {
        host.to_string()
    } else {
        format!("{host}.snowflakecomputing.com")
    }
}

impl SnowflakeSession {
    fn statement(&mut self, sql: &str, database: &str) -> AppResult<JsonValue> {
        let url = format!("https://{}/api/v2/statements", self.host);
        let body = json!({
            "statement": sql,
            "timeout": 40,
            "warehouse": if self.warehouse.is_empty() { JsonValue::Null } else { JsonValue::String(self.warehouse.clone()) },
            "database": if database.is_empty() { JsonValue::Null } else { JsonValue::String(database.to_string()) },
            "schema": self.schema,
        });
        let response = ureq::post(&url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("X-Snowflake-Authorization-Token-Type", "PROGRAMMATIC_ACCESS_TOKEN")
            .set("Content-Type", "application/json")
            .set("Accept", "application/json")
            .send_string(&body.to_string())
            .map_err(|err| AppError::msg(format!("Snowflake 请求失败: {err}")))?;
        let text = response.into_string().map_err(|err| AppError::msg(err.to_string()))?;
        serde_json::from_str(&text).map_err(|err| AppError::msg(format!("Snowflake 响应无法解析: {err} {text}")))
    }
}

impl DbSession for SnowflakeSession {
    fn engine(&self) -> Engine {
        Engine::Snowflake
    }

    fn version(&mut self) -> AppResult<String> {
        let doc = self.statement("SELECT CURRENT_VERSION()", &self.database.clone())?;
        let cell = doc
            .pointer("/data/0/0")
            .and_then(|value| value.as_str())
            .unwrap_or("Snowflake");
        Ok(format!("Snowflake {cell}"))
    }

    fn list_databases(&mut self) -> AppResult<Vec<String>> {
        let doc = self.statement("SHOW DATABASES", "")?;
        Ok(column_values(&doc, 1))
    }

    fn list_schemas(&mut self, database: &str) -> AppResult<Vec<String>> {
        let doc = self.statement(&format!("SHOW SCHEMAS IN DATABASE {database}"), database)?;
        Ok(column_values(&doc, 1))
    }

    fn list_tables(&mut self, database: &str, schema: &str) -> AppResult<Vec<String>> {
        let schema = if schema.is_empty() { self.schema.as_str() } else { schema };
        let doc = self.statement(&format!("SHOW TABLES IN SCHEMA {database}.{schema}"), database)?;
        Ok(column_values(&doc, 1))
    }

    fn list_views(&mut self, database: &str, schema: &str) -> AppResult<Vec<String>> {
        let schema = if schema.is_empty() { self.schema.as_str() } else { schema };
        let doc = self.statement(&format!("SHOW VIEWS IN SCHEMA {database}.{schema}"), database)?;
        Ok(column_values(&doc, 1))
    }

    fn list_columns(&mut self, database: &str, schema: &str, object_name: &str) -> AppResult<Vec<ColumnMeta>> {
        let schema = if schema.is_empty() { self.schema.clone() } else { schema.to_string() };
        let doc = self.statement(&format!("DESCRIBE TABLE {database}.{schema}.{object_name}"), database)?;
        let rows = doc.get("data").and_then(|value| value.as_array()).cloned().unwrap_or_default();
        Ok(rows
            .iter()
            .filter_map(|row| row.as_array())
            .map(|row| ColumnMeta {
                name: row.first().and_then(|value| value.as_str()).unwrap_or("").to_string(),
                data_type: row.get(1).and_then(|value| value.as_str()).unwrap_or("").to_string(),
                nullable: row.get(3).and_then(|value| value.as_str()).unwrap_or("Y") != "N",
                primary_key: row.get(5).and_then(|value| value.as_str()).unwrap_or("").contains('Y'),
                auto_increment: false,
                default_value: None,
            })
            .filter(|column| !column.name.is_empty())
            .collect())
    }

    fn relation_kind(&mut self, database: &str, schema: &str, object_name: &str) -> AppResult<Option<String>> {
        let tables = self.list_tables(database, schema)?;
        if tables.iter().any(|name| name.eq_ignore_ascii_case(object_name)) {
            return Ok(Some("table".into()));
        }
        let views = self.list_views(database, schema)?;
        if views.iter().any(|name| name.eq_ignore_ascii_case(object_name)) {
            Ok(Some("view".into()))
        } else {
            Ok(None)
        }
    }

    fn execute(&mut self, database: &str, sql: &str, max_rows: Option<usize>) -> AppResult<QueryOutcome> {
        let doc = self.statement(sql, database)?;
        if let Some(message) = doc.get("message").and_then(|value| value.as_str()) {
            if doc.get("data").is_none() && doc.get("code").is_some() {
                return Err(AppError::msg(message.to_string()));
            }
        }
        let names = doc
            .pointer("/resultSetMetaData/rowType")
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("name").and_then(|value| value.as_str()).map(|text| text.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut rows = doc
            .get("data")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|row| row.as_array().cloned())
            .map(|row| row.into_iter().map(snow_cell).collect::<Vec<_>>())
            .collect::<Vec<_>>();
        let truncated = max_rows.is_some_and(|limit| rows.len() > limit);
        if let Some(limit) = max_rows {
            rows.truncate(limit);
        }
        let affected = doc
            .pointer("/stats/numRowsInserted")
            .or_else(|| doc.pointer("/stats/numRowsUpdated"))
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        Ok(QueryOutcome {
            original_rows: None,
            columns: names.iter().cloned().map(ColumnMeta::named).collect(),
            rows,
            rows_affected: affected,
            truncated,
        })
    }
}

fn column_values(doc: &JsonValue, index: usize) -> Vec<String> {
    doc.get("data")
        .and_then(|value| value.as_array())
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row.as_array().and_then(|cells| cells.get(index)).and_then(|value| value.as_str()).map(|text| text.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn snow_cell(value: JsonValue) -> JsonValue {
    match value {
        JsonValue::String(text) if text.eq_ignore_ascii_case("null") => JsonValue::Null,
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_accepts_account_or_full_name() {
        assert_eq!(snowflake_host("xy12345.ap-southeast-1"), "xy12345.ap-southeast-1.snowflakecomputing.com");
        assert_eq!(snowflake_host("https://acme.snowflakecomputing.com/"), "acme.snowflakecomputing.com");
    }
}
