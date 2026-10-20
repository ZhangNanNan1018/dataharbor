use std::time::Duration;

use futures::TryStreamExt;
use serde_json::{Number, Value as JsonValue};
use tiberius::{AuthMethod, Client, Config, EncryptionLevel, QueryItem};
use tokio::net::TcpStream;
use tokio_util::compat::TokioAsyncWriteCompatExt;

use super::runtime::block_on;
use super::DbSession;
use crate::error::{AppError, AppResult};
use crate::models::{ColumnMeta, Engine, QueryOutcome, SessionConfig};
use crate::sqlutil::{quote_ident, sql_literal};

pub struct MssqlSession {
    client: Client<tokio_util::compat::Compat<TcpStream>>,
    current_db: String,
}

#[cfg(any(test, not(windows)))]
fn windows_auth_error() -> AppError {
    if cfg!(windows) {
        AppError::msg(
            "此构建没有启用 tiberius 的 winauth 特性，不能使用 Windows 身份验证。请改用 SQL Server 账号，或在 Windows 上重新编译并打开 winauth。",
        )
    } else {
        AppError::msg(
            "Windows 身份验证使用 tiberius 的 AuthMethod::Integrated（SSPI）。这个变体只在 cfg(windows) 且启用 winauth 时编译。macOS 与 Linux 的 Cargo 特性列表里有 winauth，但 tiberius 把 winauth crate 限制在 Windows，所以这里不能做 SSPI 握手。请改用 SQL Server 账号和密码。",
        )
    }
}

// `AuthMethod::Integrated` exists only when tiberius itself is built with `winauth`,
// and that variant is cfg(windows) inside tiberius. The dependency feature is set
// in Cargo.toml; this cfg is the target, not a feature of dataharbor.
#[cfg(windows)]
fn windows_auth() -> AppResult<AuthMethod> {
    Ok(AuthMethod::Integrated)
}

#[cfg(not(windows))]
fn windows_auth() -> AppResult<AuthMethod> {
    Err(windows_auth_error())
}

pub fn connect(config: &SessionConfig) -> AppResult<MssqlSession> {
    let auth = if config.auth_method.eq_ignore_ascii_case("windows") {
        windows_auth()?
    } else {
        AuthMethod::sql_server(&config.username, &config.password)
    };
    block_on(async {
        let mut tds = Config::new();
        // Tiberius validates TLS against its host, independently of our TCP dial.
        tds.host(config.tls_server_name.as_deref().unwrap_or(config.host.trim()));
        tds.port(if config.port == 0 { 1433 } else { config.port });
        tds.authentication(auth);
        if !config.database.trim().is_empty() {
            tds.database(config.database.trim());
        }
        if config.use_ssl {
            tds.encryption(EncryptionLevel::Required);
            if config.ssl_insecure {
                tds.trust_cert();
            }
        } else {
            tds.encryption(EncryptionLevel::NotSupported);
        }
        let port = if config.port == 0 { 1433 } else { config.port };
        let tcp = tokio::time::timeout(Duration::from_secs(8), TcpStream::connect((config.host.trim(), port)))
            .await
            .map_err(|_| AppError::msg("SQL Server 连接超时"))?
            .map_err(|err| AppError::msg(format!("SQL Server 连接失败: {err}")))?;
        tcp.set_nodelay(true).ok();
        let client = Client::connect(tds, tcp.compat_write())
            .await
            .map_err(|err| AppError::msg(format!("SQL Server 登录失败: {err}。若服务器强制加密，请打开 SSL。")))?;
        Ok(MssqlSession {
            client,
            current_db: config.database.trim().to_string(),
        })
    })
}

impl MssqlSession {
    fn use_db(&mut self, database: &str) -> AppResult<()> {
        if database.is_empty() || database == self.current_db {
            return Ok(());
        }
        let literal = quote_ident(Engine::Mssql, database)?;
        self.execute("", &format!("USE {literal}"), None)?;
        self.current_db = database.to_string();
        Ok(())
    }
}

impl DbSession for MssqlSession {
    fn engine(&self) -> Engine {
        Engine::Mssql
    }

    fn version(&mut self) -> AppResult<String> {
        let outcome = self.execute("", "SELECT @@VERSION", Some(1))?;
        outcome
            .rows
            .first()
            .and_then(|row| row.first())
            .and_then(|value| value.as_str())
            .map(|text| text.lines().next().unwrap_or(text).to_string())
            .ok_or_else(|| AppError::msg("无法读取 SQL Server 版本"))
    }

    fn list_databases(&mut self) -> AppResult<Vec<String>> {
        self.one_column("SELECT name FROM sys.databases ORDER BY name")
    }

    fn list_schemas(&mut self, database: &str) -> AppResult<Vec<String>> {
        self.use_db(database)?;
        self.one_column("SELECT name FROM sys.schemas ORDER BY name")
    }

    fn list_tables(&mut self, database: &str, schema: &str) -> AppResult<Vec<String>> {
        self.use_db(database)?;
        let schema = sql_literal(Engine::Mssql, &JsonValue::String(if schema.is_empty() { "dbo".into() } else { schema.into() }))?;
        self.one_column(&format!(
            "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_TYPE = 'BASE TABLE' AND TABLE_SCHEMA = {schema} ORDER BY TABLE_NAME"
        ))
    }

    fn list_views(&mut self, database: &str, schema: &str) -> AppResult<Vec<String>> {
        self.use_db(database)?;
        let schema = sql_literal(Engine::Mssql, &JsonValue::String(if schema.is_empty() { "dbo".into() } else { schema.into() }))?;
        self.one_column(&format!(
            "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.VIEWS WHERE TABLE_SCHEMA = {schema} ORDER BY TABLE_NAME"
        ))
    }

    fn list_columns(&mut self, database: &str, schema: &str, object_name: &str) -> AppResult<Vec<ColumnMeta>> {
        self.use_db(database)?;
        let schema_lit = sql_literal(Engine::Mssql, &JsonValue::String(if schema.is_empty() { "dbo".into() } else { schema.into() }))?;
        let table_lit = sql_literal(Engine::Mssql, &JsonValue::String(object_name.to_string()))?;
        let outcome = self.execute(
            "",
            &format!(
                "SELECT c.COLUMN_NAME, c.DATA_TYPE, c.IS_NULLABLE, CASE WHEN k.COLUMN_NAME IS NULL THEN 0 ELSE 1 END, COLUMNPROPERTY(OBJECT_ID(c.TABLE_SCHEMA + '.' + c.TABLE_NAME), c.COLUMN_NAME, 'IsIdentity'), c.COLUMN_DEFAULT
                 FROM INFORMATION_SCHEMA.COLUMNS c
                 LEFT JOIN INFORMATION_SCHEMA.KEY_COLUMN_USAGE k
                   ON k.TABLE_SCHEMA = c.TABLE_SCHEMA AND k.TABLE_NAME = c.TABLE_NAME AND k.COLUMN_NAME = c.COLUMN_NAME
                   AND k.CONSTRAINT_NAME IN (SELECT CONSTRAINT_NAME FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS WHERE CONSTRAINT_TYPE = 'PRIMARY KEY')
                 WHERE c.TABLE_SCHEMA = {schema_lit} AND c.TABLE_NAME = {table_lit}
                 ORDER BY c.ORDINAL_POSITION"
            ),
            Some(500),
        )?;
        Ok(outcome
            .rows
            .into_iter()
            .map(|row| ColumnMeta {
                name: row.first().and_then(|value| value.as_str()).unwrap_or("").to_string(),
                data_type: row.get(1).and_then(|value| value.as_str()).unwrap_or("").to_string(),
                nullable: row.get(2).and_then(|value| value.as_str()).map(|value| value.eq_ignore_ascii_case("YES")).unwrap_or(true),
                primary_key: row.get(3).and_then(json_flag).unwrap_or(false),
                auto_increment: row.get(4).and_then(json_flag).unwrap_or(false),
                default_value: row.get(5).and_then(|value| value.as_str()).map(|text| text.to_string()),
            })
            .filter(|column| !column.name.is_empty())
            .collect())
    }

    fn relation_kind(&mut self, database: &str, schema: &str, object_name: &str) -> AppResult<Option<String>> {
        self.use_db(database)?;
        let schema_lit = sql_literal(Engine::Mssql, &JsonValue::String(schema.to_string()))?;
        let name_lit = sql_literal(Engine::Mssql, &JsonValue::String(object_name.to_string()))?;
        let outcome = self.execute(
            "",
            &format!("SELECT TABLE_TYPE FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = {schema_lit} AND TABLE_NAME = {name_lit}"),
            Some(1),
        )?;
        Ok(outcome.rows.first().and_then(|row| row.first()).and_then(|value| value.as_str()).map(|text| {
            if text.eq_ignore_ascii_case("VIEW") { "view".into() } else { "table".into() }
        }))
    }

    fn execute(&mut self, database: &str, sql: &str, max_rows: Option<usize>) -> AppResult<QueryOutcome> {
        if !database.is_empty() {
            self.use_db(database)?;
        }
        let sql = sql.to_string();
        let max_rows = max_rows.unwrap_or(5_000);
        block_on(async {
            let mut stream = self.client.query(&sql, &[]).await.map_err(|err| AppError::msg(err.to_string()))?;
            let mut columns = Vec::new();
            let mut rows = Vec::new();
            let mut truncated = false;
            while let Some(item) = stream.try_next().await.map_err(|err| AppError::msg(err.to_string()))? {
                match item {
                    QueryItem::Metadata(meta) => {
                        if columns.is_empty() {
                            columns = meta
                                .columns()
                                .iter()
                                .map(|column| ColumnMeta {
                                    name: column.name().to_string(),
                                    data_type: format!("{:?}", column.column_type()),
                                    nullable: true,
                                    primary_key: false,
                                    auto_increment: false,
                                    default_value: None,
                                })
                                .collect();
                        }
                    }
                    QueryItem::Row(row) => {
                        if rows.len() >= max_rows {
                            truncated = true;
                            continue;
                        }
                        let mut cells = Vec::new();
                        for index in 0..columns.len().max(row.len()) {
                            cells.push(cell_value(&row, index));
                        }
                        rows.push(cells);
                    }
                }
            }
            let affected = if columns.is_empty() { rows.len() as u64 } else { 0 };
            Ok(QueryOutcome {
                columns,
                rows,
                rows_affected: affected,
                truncated,
            })
        })
    }
}

impl MssqlSession {
    fn one_column(&mut self, sql: &str) -> AppResult<Vec<String>> {
        let outcome = self.execute("", sql, Some(5_000))?;
        Ok(outcome
            .rows
            .into_iter()
            .filter_map(|row| row.into_iter().next())
            .map(|value| match value {
                JsonValue::String(text) => text,
                other => other.to_string(),
            })
            .collect())
    }
}

fn json_flag(value: &JsonValue) -> Option<bool> {
    match value {
        JsonValue::Number(number) => Some(number.as_i64().unwrap_or(0) != 0),
        JsonValue::Bool(flag) => Some(*flag),
        JsonValue::String(text) => Some(text == "1" || text.eq_ignore_ascii_case("true")),
        _ => None,
    }
}

fn cell_value(row: &tiberius::Row, index: usize) -> JsonValue {
    macro_rules! pull {
        ($ty:ty, $map:expr) => {
            match row.try_get::<$ty, _>(index) {
                Ok(Some(value)) => return $map(value),
                Ok(None) => return JsonValue::Null,
                Err(_) => {}
            }
        };
    }
    pull!(&str, |value: &str| JsonValue::String(value.to_string()));
    pull!(i32, |value: i32| JsonValue::Number(value.into()));
    pull!(i64, |value: i64| Number::from_i128(i128::from(value)).map(JsonValue::Number).unwrap_or_else(|| JsonValue::String(value.to_string())));
    pull!(f64, |value: f64| Number::from_f64(value).map(JsonValue::Number).unwrap_or(JsonValue::Null));
    pull!(bool, JsonValue::Bool);
    pull!(chrono::NaiveDateTime, |value: chrono::NaiveDateTime| JsonValue::String(value.to_string()));
    pull!(chrono::NaiveDate, |value: chrono::NaiveDate| JsonValue::String(value.to_string()));
    pull!(chrono::NaiveTime, |value: chrono::NaiveTime| JsonValue::String(value.to_string()));
    JsonValue::Null
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_auth_is_compiled_only_with_sspi() {
        let text = windows_auth_error().to_string();
        assert!(text.contains("winauth") || text.contains("AuthMethod::Integrated"));
        #[cfg(not(windows))]
        {
            let err = windows_auth().unwrap_err().to_string();
            assert!(err.contains("AuthMethod::Integrated"));
            assert!(err.contains("cfg(windows)"));
        }
        #[cfg(windows)]
        {
            let method = windows_auth().expect("SSPI auth method");
            let _ = method;
        }
    }
}
