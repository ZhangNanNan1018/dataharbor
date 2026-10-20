use std::collections::HashMap;
use std::time::Duration;

use native_tls::TlsConnector;
use postgres::config::SslMode;
use postgres::{Client, Config, NoTls, SimpleQueryMessage};
use postgres_native_tls::MakeTlsConnector;
use serde_json::Value as JsonValue;

use super::DbSession;
use crate::error::{AppError, AppResult};
use crate::models::{ColumnMeta, Engine, QueryOutcome, SessionConfig};
use crate::sqlutil::sql_literal;

pub struct PostgresSession {
    clients: HashMap<String, Client>,
    host: String,
    port: u16,
    username: String,
    password: String,
    use_ssl: bool,
    ssl_insecure: bool,
    tls_server_name: Option<String>,
    default_db: String,
}

/// rust-postgres 0.19 sits on tokio-postgres 0.7. That stack parses
/// AuthenticationGss (auth code 7) and AuthenticationGssContinue, then
/// `connect_raw` returns "unsupported authentication method" without calling
/// gss_init_sec_context. tiberius `integrated-auth-gssapi` is a TDS login, not
/// a PostgreSQL startup packet, so it is not a substitute. libpq is not linked.
pub fn gssapi_refusal() -> &'static str {
    "当前 PostgreSQL 驱动不支持 GSSAPI / Kerberos。rust-postgres 0.19 / tokio-postgres 0.7 会解析 AuthenticationGss（认证码 7）和 AuthenticationGssContinue，但 connect_raw 把它们当成不支持的认证方法直接返回，没有 GSS token 交换，也不会调用 gss_init_sec_context。tiberius 的 integrated-auth-gssapi 只用于 SQL Server TDS 登录，不能用于 PostgreSQL 启动包。本程序不链接 libpq。请改用密码、md5 或 SCRAM-SHA-256。"
}

pub fn connect(config: &SessionConfig) -> AppResult<PostgresSession> {
    if config.auth_method.eq_ignore_ascii_case("gssapi") {
        return Err(AppError::msg(gssapi_refusal()));
    }
    let database = if config.database.trim().is_empty() {
        "postgres".to_string()
    } else {
        config.database.trim().to_string()
    };
    let mut session = PostgresSession {
        clients: HashMap::new(),
        host: config.host.trim().to_string(),
        port: if config.port == 0 {
            config.engine.default_port()
        } else {
            config.port
        },
        username: config.username.clone(),
        password: config.password.clone(),
        use_ssl: config.use_ssl,
        ssl_insecure: config.ssl_insecure,
        tls_server_name: config.tls_server_name.clone(),
        default_db: database.clone(),
    };
    let client = open_client(
        &session.host,
        session.port,
        &session.username,
        &session.password,
        &database,
        session.use_ssl,
        session.ssl_insecure,
        session.tls_server_name.as_deref(),
    )?;
    session.clients.insert(database, client);
    Ok(session)
}

pub fn open_configured(config: &SessionConfig, database: &str) -> AppResult<Client> {
    let database = if database.trim().is_empty() {
        if config.database.trim().is_empty() {
            "postgres"
        } else {
            config.database.trim()
        }
    } else {
        database.trim()
    };
    let port = if config.port == 0 { 5432 } else { config.port };
    open_client(
        config.host.trim(),
        port,
        &config.username,
        &config.password,
        database,
        config.use_ssl,
        config.ssl_insecure,
        config.tls_server_name.as_deref(),
    )
}

pub fn open_client(
    host: &str,
    port: u16,
    username: &str,
    password: &str,
    database: &str,
    use_ssl: bool,
    ssl_insecure: bool,
    tls_server_name: Option<&str>,
) -> AppResult<Client> {
    let mut config = connection_options(host, port, use_ssl, tls_server_name)?;
    config
        .user(username)
        .password(password)
        .dbname(database)
        .connect_timeout(Duration::from_secs(8))
        .application_name("dataharbor");
    let client = if use_ssl {
        let mut builder = TlsConnector::builder();
        if ssl_insecure {
            builder.danger_accept_invalid_certs(true);
            builder.danger_accept_invalid_hostnames(true);
        }
        let connector = builder
            .build()
            .map_err(|err| AppError::msg(format!("TLS 初始化失败: {err}")))?;
        config
            .connect(MakeTlsConnector::new(connector))
            .map_err(|err| AppError::msg(err.to_string()))?
    } else {
        config
            .connect(NoTls)
            .map_err(|err| AppError::msg(err.to_string()))?
    };
    Ok(client)
}

fn connection_options(host: &str, port: u16, use_ssl: bool, tls_server_name: Option<&str>) -> AppResult<Config> {
    let mut config = Config::new();
    config.port(port).ssl_mode(if use_ssl { SslMode::Require } else { SslMode::Disable });
    if let Some(server_name) = tls_server_name {
        // host remains the remote TLS/SNI identity; hostaddr is the local tunnel.
        let address = host.parse().map_err(|_| AppError::msg("PostgreSQL 隧道的本地地址必须是 IP"))?;
        config.host(server_name).hostaddr(address);
    } else {
        config.host(host);
    }
    Ok(config)
}

impl PostgresSession {
    fn open_client(&self, database: &str) -> AppResult<Client> {
        open_client(
            &self.host,
            self.port,
            &self.username,
            &self.password,
            database,
            self.use_ssl,
            self.ssl_insecure,
            self.tls_server_name.as_deref(),
        )
    }

    fn resolve_db(&self, database: &str) -> String {
        let database = database.trim();
        if database.is_empty() {
            self.default_db.clone()
        } else {
            database.to_string()
        }
    }

    fn with_client<T>(
        &mut self,
        database: &str,
        f: impl FnOnce(&mut Client) -> AppResult<T>,
    ) -> AppResult<T> {
        let database = self.resolve_db(database);
        if !self.clients.contains_key(&database) {
            let client = self.open_client(&database)?;
            self.clients.insert(database.clone(), client);
        }
        let client = self
            .clients
            .get_mut(&database)
            .ok_or_else(|| AppError::msg("无法打开 PostgreSQL 连接"))?;
        f(client)
    }

    fn one_column(&mut self, database: &str, sql: &str) -> AppResult<Vec<String>> {
        let outcome = self.execute(database, sql, None)?;
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

impl DbSession for PostgresSession {
    fn engine(&self) -> Engine {
        Engine::Postgres
    }

    fn version(&mut self) -> AppResult<String> {
        let values = self.one_column("", "SELECT version()")?;
        values
            .into_iter()
            .next()
            .ok_or_else(|| AppError::msg("无法读取服务器版本"))
    }

    fn list_databases(&mut self) -> AppResult<Vec<String>> {
        self.one_column(
            "",
            "SELECT datname FROM pg_database WHERE datistemplate = false AND datallowconn = true ORDER BY datname",
        )
    }

    fn list_schemas(&mut self, database: &str) -> AppResult<Vec<String>> {
        self.one_column(
            database,
            "SELECT nspname FROM pg_namespace \
             WHERE nspname NOT IN ('pg_catalog', 'information_schema') \
               AND nspname NOT LIKE 'pg_toast%' \
               AND nspname NOT LIKE 'pg_temp%' \
             ORDER BY nspname",
        )
    }

    fn list_tables(&mut self, database: &str, schema: &str) -> AppResult<Vec<String>> {
        let schema = schema_or_public(schema);
        let literal = sql_literal(Engine::Postgres, &JsonValue::String(schema))?;
        self.one_column(
            database,
            &format!(
                "SELECT table_name FROM information_schema.tables \
                 WHERE table_schema = {literal} AND table_type = 'BASE TABLE' ORDER BY table_name"
            ),
        )
    }

    fn list_views(&mut self, database: &str, schema: &str) -> AppResult<Vec<String>> {
        let schema = schema_or_public(schema);
        let literal = sql_literal(Engine::Postgres, &JsonValue::String(schema))?;
        self.one_column(
            database,
            &format!(
                "SELECT table_name FROM information_schema.tables \
                 WHERE table_schema = {literal} AND table_type = 'VIEW' ORDER BY table_name"
            ),
        )
    }

    fn list_columns(
        &mut self,
        database: &str,
        schema: &str,
        object_name: &str,
    ) -> AppResult<Vec<ColumnMeta>> {
        let schema = schema_or_public(schema);
        let schema_lit = sql_literal(Engine::Postgres, &JsonValue::String(schema))?;
        let table_lit = sql_literal(Engine::Postgres, &JsonValue::String(object_name.to_string()))?;
        let sql = format!(
            "SELECT c.column_name, c.data_type, c.is_nullable, c.column_default, \
                    CASE WHEN EXISTS ( \
                        SELECT 1 FROM information_schema.table_constraints tc \
                        JOIN information_schema.key_column_usage kcu \
                          ON tc.constraint_name = kcu.constraint_name \
                         AND tc.table_schema = kcu.table_schema \
                         AND tc.table_name = kcu.table_name \
                        WHERE tc.constraint_type = 'PRIMARY KEY' \
                          AND tc.table_schema = c.table_schema \
                          AND tc.table_name = c.table_name \
                          AND kcu.column_name = c.column_name \
                    ) THEN 'YES' ELSE 'NO' END AS is_pk \
             FROM information_schema.columns c \
             WHERE c.table_schema = {schema_lit} AND c.table_name = {table_lit} \
             ORDER BY c.ordinal_position"
        );
        let outcome = self.execute(database, &sql, None)?;
        Ok(outcome
            .rows
            .into_iter()
            .map(|row| {
                let text = |index: usize| match row.get(index) {
                    Some(JsonValue::String(value)) => value.clone(),
                    Some(JsonValue::Null) | None => String::new(),
                    Some(other) => other.to_string(),
                };
                let default_value = text(3);
                let auto = default_value.to_ascii_lowercase().starts_with("nextval(");
                ColumnMeta {
                    name: text(0),
                    data_type: text(1),
                    nullable: text(2).eq_ignore_ascii_case("YES"),
                    default_value: if default_value.is_empty() {
                        None
                    } else {
                        Some(default_value)
                    },
                    primary_key: text(4).eq_ignore_ascii_case("YES"),
                    auto_increment: auto,
                }
            })
            .collect())
    }

    fn relation_kind(
        &mut self,
        database: &str,
        schema: &str,
        object_name: &str,
    ) -> AppResult<Option<String>> {
        let schema = schema_or_public(schema);
        let schema_lit = sql_literal(Engine::Postgres, &JsonValue::String(schema))?;
        let table_lit = sql_literal(Engine::Postgres, &JsonValue::String(object_name.to_string()))?;
        let rows = self.one_column(
            database,
            &format!(
                "SELECT table_type FROM information_schema.tables \
                 WHERE table_schema = {schema_lit} AND table_name = {table_lit}"
            ),
        )?;
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
        self.with_client(database, |client| run_simple(client, sql, max_rows))
    }
}

fn schema_or_public(schema: &str) -> String {
    if schema.trim().is_empty() {
        "public".to_string()
    } else {
        schema.trim().to_string()
    }
}

pub(crate) fn run_simple(client: &mut Client, sql: &str, max_rows: Option<usize>) -> AppResult<QueryOutcome> {
    let messages = client
        .simple_query(sql)
        .map_err(|err| AppError::msg(err.to_string()))?;
    let mut columns: Vec<ColumnMeta> = Vec::new();
    let mut rows = Vec::new();
    let mut affected = 0u64;
    let mut truncated = false;
    let mut saw_rows = false;
    for message in messages {
        match message {
            SimpleQueryMessage::RowDescription(description) => {
                if columns.is_empty() {
                    columns = description
                        .iter()
                        .map(|column| ColumnMeta::named(column.name()))
                        .collect();
                }
            }
            SimpleQueryMessage::Row(row) => {
                saw_rows = true;
                if columns.is_empty() {
                    columns = row
                        .columns()
                        .iter()
                        .map(|column| ColumnMeta::named(column.name()))
                        .collect();
                }
                if max_rows.is_some_and(|limit| rows.len() >= limit) {
                    truncated = true;
                    continue;
                }
                let mut cells = Vec::with_capacity(row.len());
                for index in 0..row.len() {
                    cells.push(match row.try_get(index) {
                        Ok(Some(text)) => JsonValue::String(text.to_string()),
                        Ok(None) => JsonValue::Null,
                        Err(err) => return Err(AppError::msg(err.to_string())),
                    });
                }
                rows.push(cells);
            }
            SimpleQueryMessage::CommandComplete(count) => {
                affected = count;
            }
            _ => {}
        }
    }
    Ok(QueryOutcome {
        columns,
        rows,
        rows_affected: if saw_rows { 0 } else { affected },
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tunneled_tls_dials_loopback_but_verifies_the_remote_host() {
        let options = connection_options("127.0.0.1", 49152, true, Some("db.example.test")).unwrap();
        assert_eq!(options.get_hosts(), &[postgres::config::Host::Tcp("db.example.test".into())]);
        assert_eq!(options.get_hostaddrs(), &["127.0.0.1".parse::<std::net::IpAddr>().unwrap()]);
        assert_eq!(options.get_ports(), &[49152]);
        assert_eq!(options.get_ssl_mode(), SslMode::Require);
    }

    #[test]
    fn selected_tls_is_required_and_never_falls_back_to_plaintext() {
        let tls = connection_options("db.example.test", 5432, true, None).unwrap();
        assert_eq!(tls.get_ssl_mode(), SslMode::Require);
        assert!(tls.get_hostaddrs().is_empty());
        let plain = connection_options("db.example.test", 5432, false, None).unwrap();
        assert_eq!(plain.get_ssl_mode(), SslMode::Disable);
        assert!(connection_options("not-a-local-ip", 49152, true, Some("db.example.test")).is_err());
    }

    #[test]
    fn gssapi_refusal_names_the_missing_exchange() {
        let text = gssapi_refusal();
        assert!(text.contains("AuthenticationGss"));
        assert!(text.contains("gss_init_sec_context"));
        assert!(text.contains("tokio-postgres"));
        assert!(text.contains("integrated-auth-gssapi"));
    }
}
