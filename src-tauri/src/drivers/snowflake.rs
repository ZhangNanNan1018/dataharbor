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
    agent: ureq::Agent,
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
        agent: ureq::AgentBuilder::new()
            .redirects(0)
            .timeout(std::time::Duration::from_secs(60))
            .build(),
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
    fn request(&self, method: &str, path: &str, body: Option<&JsonValue>) -> AppResult<(u16, JsonValue)> {
        let url = format!("https://{}/api/v2/statements{path}", self.host);
        let request = self.agent.request(method, &url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("X-Snowflake-Authorization-Token-Type", "PROGRAMMATIC_ACCESS_TOKEN")
            .set("Content-Type", "application/json")
            .set("Accept", "application/json");
        let response = match body {
            Some(body) => request.send_string(&body.to_string()),
            None => request.call(),
        };
        let response = match response {
            Ok(response) | Err(ureq::Error::Status(429, response)) => response,
            Err(error) => return Err(AppError::msg(format!("Snowflake 请求失败: {error}"))),
        };
        let status = response.status();
        let text = response.into_string().map_err(|err| AppError::msg(err.to_string()))?;
        let document = serde_json::from_str(&text)
            .map_err(|err| AppError::msg(format!("Snowflake 响应无法解析: {err}")))?;
        Ok((status, document))
    }

    fn statement(&mut self, sql: &str, database: &str, max_rows: Option<usize>) -> AppResult<SnowflakeResult> {
        let body = json!({
            "statement": sql,
            "timeout": 40,
            "warehouse": if self.warehouse.is_empty() { JsonValue::Null } else { JsonValue::String(self.warehouse.clone()) },
            "database": if database.is_empty() { JsonValue::Null } else { JsonValue::String(database.to_string()) },
            "schema": self.schema,
        });
        let (status, document) = self.request("POST", "", Some(&body))?;
        let document = finish_statement(status, document, |handle| self.request("GET", &format!("/{handle}"), None))?;
        collect_partitions(document, max_rows, |handle, index| {
            let (status, document) = self.request("GET", &format!("/{handle}?partition={index}"), None)?;
            if status != 200 {
                return Err(AppError::msg(format!("Snowflake 分区 {index} 尚未就绪（HTTP {status}）；没有把不完整结果当作成功")));
            }
            Ok(document)
        })
    }
}

struct SnowflakeResult {
    document: JsonValue,
    truncated: bool,
}

fn statement_handle(document: &JsonValue) -> AppResult<&str> {
    document.get("statementHandle").and_then(JsonValue::as_str)
        .filter(|handle| !handle.is_empty() && handle.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'))
        .ok_or_else(|| AppError::msg("Snowflake 未返回有效的 statementHandle，无法确认查询结果；请核对服务端状态后再操作"))
}

fn finish_statement(
    mut status: u16,
    mut document: JsonValue,
    mut poll: impl FnMut(&str) -> AppResult<(u16, JsonValue)>,
) -> AppResult<JsonValue> {
    let started = std::time::Instant::now();
    while matches!(status, 202 | 429) {
        let handle = statement_handle(&document)?.to_string();
        if started.elapsed() > std::time::Duration::from_secs(60) {
            return Err(AppError::msg(format!("Snowflake 仍在执行（{handle}）；结果尚未确认，请核对服务端状态，勿重复执行写操作")));
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
        (status, document) = poll(&handle)?;
    }
    if status != 200 {
        return Err(AppError::msg(format!("Snowflake 返回 HTTP {status}，结果尚未确认")));
    }
    Ok(document)
}

/// Fetch API partitions, respecting the caller's cap while retaining a truthful
/// truncation flag. API JSON null is SQL NULL; the text "null" stays text.
fn collect_partitions(
    mut document: JsonValue,
    max_rows: Option<usize>,
    mut fetch: impl FnMut(&str, usize) -> AppResult<JsonValue>,
) -> AppResult<SnowflakeResult> {
    let metadata = document.get("resultSetMetaData")
        .ok_or_else(|| AppError::msg("Snowflake 未返回结果元数据，无法确认完整性"))?;
    let total = metadata.get("numRows").and_then(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()))
        .ok_or_else(|| AppError::msg("Snowflake 未返回结果总行数，无法确认完整性"))?;
    let partitions = metadata.get("partitionInfo").and_then(JsonValue::as_array).cloned().unwrap_or_default();
    let mut rows = take_partition_rows(&mut document)?;
    let mut loaded = rows.len() as u64;
    if let Some(expected) = partitions.first().and_then(|partition| partition.get("rowCount")).and_then(JsonValue::as_u64) {
        if expected != loaded {
            return Err(AppError::msg("Snowflake 首分区行数不完整，已停止读取"));
        }
    }
    if let Some(limit) = max_rows { rows.truncate(limit); }
    for index in 1..partitions.len() {
        if max_rows.is_some_and(|limit| rows.len() >= limit) { break; }
        let handle = statement_handle(&document)?;
        let mut partition = fetch(handle, index)?;
        let mut next = take_partition_rows(&mut partition)?;
        if let Some(expected) = partitions[index].get("rowCount").and_then(JsonValue::as_u64) {
            if expected != next.len() as u64 {
                return Err(AppError::msg(format!("Snowflake 分区 {index} 行数不完整，已停止读取")));
            }
        }
        loaded += next.len() as u64;
        if let Some(limit) = max_rows { next.truncate(limit.saturating_sub(rows.len())); }
        rows.extend(next);
    }
    let capped = max_rows.is_some_and(|limit| rows.len() >= limit && (rows.len() as u64) < total);
    if loaded > total || (!capped && loaded != total) {
        return Err(AppError::msg(format!("Snowflake 返回结果不完整（声明 {total} 行，收到 {loaded} 行）；未将首分区当作完整结果")));
    }
    let truncated = (rows.len() as u64) < total;
    document["data"] = JsonValue::Array(rows);
    Ok(SnowflakeResult { document, truncated })
}

fn take_partition_rows(document: &mut JsonValue) -> AppResult<Vec<JsonValue>> {
    let value = if document.is_array() { document.take() } else { document.get_mut("data").map(JsonValue::take).unwrap_or(JsonValue::Null) };
    match value {
        JsonValue::Array(rows) if rows.iter().all(JsonValue::is_array) => Ok(rows),
        _ => Err(AppError::msg("Snowflake 未返回有效数据行，结果尚未确认")),
    }
}

impl DbSession for SnowflakeSession {
    fn engine(&self) -> Engine {
        Engine::Snowflake
    }

    fn version(&mut self) -> AppResult<String> {
        let doc = self.statement("SELECT CURRENT_VERSION()", &self.database.clone(), Some(1))?.document;
        let cell = doc
            .pointer("/data/0/0")
            .and_then(|value| value.as_str())
            .unwrap_or("Snowflake");
        Ok(format!("Snowflake {cell}"))
    }

    fn list_databases(&mut self) -> AppResult<Vec<String>> {
        let doc = self.statement("SHOW DATABASES", "", None)?.document;
        Ok(column_values(&doc, 1))
    }

    fn list_schemas(&mut self, database: &str) -> AppResult<Vec<String>> {
        let doc = self.statement(&format!("SHOW SCHEMAS IN DATABASE {database}"), database, None)?.document;
        Ok(column_values(&doc, 1))
    }

    fn list_tables(&mut self, database: &str, schema: &str) -> AppResult<Vec<String>> {
        let schema = if schema.is_empty() { self.schema.as_str() } else { schema };
        let doc = self.statement(&format!("SHOW TABLES IN SCHEMA {database}.{schema}"), database, None)?.document;
        Ok(column_values(&doc, 1))
    }

    fn list_views(&mut self, database: &str, schema: &str) -> AppResult<Vec<String>> {
        let schema = if schema.is_empty() { self.schema.as_str() } else { schema };
        let doc = self.statement(&format!("SHOW VIEWS IN SCHEMA {database}.{schema}"), database, None)?.document;
        Ok(column_values(&doc, 1))
    }

    fn list_columns(&mut self, database: &str, schema: &str, object_name: &str) -> AppResult<Vec<ColumnMeta>> {
        let schema = if schema.is_empty() { self.schema.clone() } else { schema.to_string() };
        let doc = self.statement(&format!("DESCRIBE TABLE {database}.{schema}.{object_name}"), database, None)?.document;
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
        let result = self.statement(sql, database, max_rows)?;
        decode_result(result)
    }
}

fn decode_result(result: SnowflakeResult) -> AppResult<QueryOutcome> {
        let mut doc = result.document;
        if let Some(message) = doc.get("message").and_then(|value| value.as_str()) {
            if doc.get("data").is_none() && doc.get("code").is_some() {
                return Err(AppError::msg(message.to_string()));
            }
        }
        let columns = doc
            .pointer("/resultSetMetaData/rowType")
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let mut column = ColumnMeta::named(item.get("name")?.as_str()?.to_string());
                        column.data_type = item.get("type").and_then(JsonValue::as_str).unwrap_or("").to_string();
                        column.nullable = item.get("nullable").and_then(JsonValue::as_bool).unwrap_or(true);
                        Some(column)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let rows = take_partition_rows(&mut doc)?
            .into_iter()
            .map(|row| match row { JsonValue::Array(cells) => cells, _ => unreachable!() })
            .collect::<Vec<_>>();
        let affected = doc
            .pointer("/stats/numRowsInserted")
            .or_else(|| doc.pointer("/stats/numRowsUpdated"))
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        Ok(QueryOutcome {
            original_rows: None,
            columns,
            rows,
            rows_affected: affected,
            truncated: result.truncated,
        })
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

#[cfg(test)]
#[path = "snowflake_result_tests.rs"]
mod result_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_accepts_account_or_full_name() {
        assert_eq!(snowflake_host("xy12345.ap-southeast-1"), "xy12345.ap-southeast-1.snowflakecomputing.com");
        assert_eq!(snowflake_host("https://acme.snowflakecomputing.com/"), "acme.snowflakecomputing.com");
    }

    fn response(total: u64, counts: &[u64], rows: JsonValue) -> JsonValue {
        json!({
            "statementHandle": "fixture-handle",
            "resultSetMetaData": {
                "numRows": total,
                "rowType": [{"name": "value", "type": "text"}],
                "partitionInfo": counts.iter().map(|count| json!({"rowCount":count})).collect::<Vec<_>>()
            },
            "data": rows,
        })
    }

    #[test]
    fn sql_null_and_null_text_remain_distinct_across_partitions() {
        let first = response(3, &[1, 2], json!([["null"]]));
        let mut calls = Vec::new();
        let result = collect_partitions(first, None, |handle, index| {
            calls.push((handle.to_string(), index));
            Ok(json!({"data": [["NULL"], [null]]}))
        }).unwrap();
        assert_eq!(calls, [("fixture-handle".into(), 1)]);
        assert_eq!(result.document["data"], json!([["null"], ["NULL"], [null]]));
        assert!(!result.truncated);
    }

    #[test]
    fn capped_partition_reads_are_reported_and_do_not_fetch_unused_partitions() {
        let first = response(4, &[2, 2], json!([["a"], ["b"]]));
        let result = collect_partitions(first, Some(1), |_, _| panic!("cap already reached")).unwrap();
        assert_eq!(result.document["data"], json!([["a"]]));
        assert!(result.truncated);

        let first = response(4, &[1, 3], json!([["a"]]));
        let result = collect_partitions(first, Some(2), |_, _| Ok(json!([["b"],["c"],["d"]]))).unwrap();
        assert_eq!(result.document["data"], json!([["a"], ["b"]]));
        assert!(result.truncated);
    }

    #[test]
    fn incomplete_or_malformed_partition_results_are_never_successful() {
        assert!(collect_partitions(response(4, &[1], json!([["a"]])), None, |_, _| unreachable!()).is_err());
        assert!(collect_partitions(response(3, &[1, 2], json!([["a"]])), None, |_, _| Ok(json!({"data":[["b"]]}))).is_err());
        assert!(collect_partitions(response(3, &[1, 2], json!([["a"]])), None, |_, _| Ok(json!({"message":"pending"}))).is_err());
        assert!(collect_partitions(json!({"data":[]}), None, |_, _| unreachable!()).is_err());
    }

    #[test]
    fn pending_statement_is_polled_before_results_are_decoded() {
        let mut calls = 0;
        let document = finish_statement(202, json!({"statementHandle":"fixture-handle"}), |handle| {
            assert_eq!(handle, "fixture-handle");
            calls += 1;
            Ok((200, response(1, &[1], json!([["ready"]]))))
        }).unwrap();
        assert_eq!(calls, 1);
        assert_eq!(document["data"], json!([["ready"]]));
        assert!(finish_statement(202, json!({"statementHandle":"../other-host"}), |_| unreachable!()).is_err());
    }
}
