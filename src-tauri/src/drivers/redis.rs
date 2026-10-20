use std::collections::HashMap;
use std::time::Duration;

use serde_json::{json, Value as JsonValue};

use super::DbSession;
use crate::error::{AppError, AppResult};
use crate::models::{ColumnMeta, Engine, FilterSpec, QueryOutcome, RowChange, SessionConfig, SortSpec};

enum RedisLink {
    Single(redis::Connection),
    Cluster(redis::cluster::ClusterConnection),
    Mapped(MappedCluster),
}

impl RedisLink {
    fn as_mut(&mut self) -> &mut dyn redis::ConnectionLike {
        match self {
            RedisLink::Single(conn) => conn,
            RedisLink::Cluster(conn) => conn,
            RedisLink::Mapped(conn) => conn,
        }
    }
}

struct NodeLink {
    dial: String,
    conn: redis::Connection,
}

struct SlotRange {
    start: u16,
    end: u16,
    index: usize,
}

struct MappedCluster {
    nodes: Vec<NodeLink>,
    slots: Vec<SlotRange>,
    map: HashMap<String, String>,
    username: String,
    password: String,
}

pub struct RedisSession {
    conn: RedisLink,
    mode: String,
}

pub fn connect(config: &SessionConfig) -> AppResult<RedisSession> {
    if config.use_ssl {
        return Err(AppError::msg("当前 Redis 构建未包含 TLS 支持，不能启用 SSL；程序不会退回明文连接。"));
    }
    let mode = config.server_mode.trim().to_ascii_lowercase();
    let conn = if mode == "cluster" {
        let nodes = config
            .host
            .split([',', ' '])
            .filter(|item| !item.trim().is_empty())
            .map(|item| {
                if item.contains("://") {
                    item.to_string()
                } else {
                    let (host, port) = split_host_port(item, config.port);
                    redis_url(&host, port, &config.username, &config.password, 0)
                }
            })
            .collect::<Vec<_>>();
        if nodes.is_empty() {
            return Err(AppError::msg("Redis Cluster 需要至少一个节点"));
        }
        let map = parse_address_map(&config.address_map);
        if map.is_empty() {
            let client = redis::cluster::ClusterClient::new(nodes).map_err(|err| AppError::msg(err.to_string()))?;
            RedisLink::Cluster(
                client
                    .get_connection()
                    .map_err(|err| AppError::msg(format!("Redis Cluster 连接失败: {err}")))?,
            )
        } else {
            RedisLink::Mapped(MappedCluster::open(nodes, map, &config.username, &config.password)?)
        }
    } else if mode == "sentinel" {
        let master = if config.database.trim().is_empty() {
            "mymaster"
        } else {
            config.database.trim()
        };
        let addr = sentinel_master(config, master)?;
        let client = redis::Client::open(addr).map_err(|err| AppError::msg(err.to_string()))?;
        let conn = client
            .get_connection()
            .map_err(|err| AppError::msg(format!("Redis Sentinel 主节点连接失败: {err}")))?;
        RedisLink::Single(conn)
    } else {
        let client = redis::Client::open(redis_url(&config.host, config.port, &config.username, &config.password, 0))
            .map_err(|err| AppError::msg(err.to_string()))?;
        let conn = client.get_connection().map_err(|err| AppError::msg(format!("Redis 连接失败: {err}")))?;
        conn.set_read_timeout(Some(Duration::from_secs(15))).ok();
        conn.set_write_timeout(Some(Duration::from_secs(15))).ok();
        RedisLink::Single(conn)
    };
    Ok(RedisSession { conn, mode })
}

fn split_host_port(item: &str, default_port: u16) -> (String, u16) {
    let item = item.trim();
    if let Some((host, port)) = item.rsplit_once(':') {
        if !host.is_empty() && !host.contains(':') {
            if let Ok(port) = port.parse::<u16>() {
                return (host.to_string(), port);
            }
        }
    }
    (item.to_string(), if default_port == 0 { 6379 } else { default_port })
}

fn redis_url(host: &str, port: u16, username: &str, password: &str, db: i64) -> String {
    let host = host.trim();
    let port = if port == 0 { 6379 } else { port };
    if password.is_empty() {
        format!("redis://{host}:{port}/{db}")
    } else if username.trim().is_empty() {
        format!("redis://:{}@{host}:{port}/{db}", encode(password))
    } else {
        format!("redis://{}:{}@{host}:{port}/{db}", encode(username.trim()), encode(password))
    }
}

fn encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn sentinel_master(config: &SessionConfig, service: &str) -> AppResult<String> {
    let client = redis::Client::open(redis_url(&config.host, config.port, &config.username, &config.password, 0))
        .map_err(|err| AppError::msg(err.to_string()))?;
    let mut conn = client.get_connection().map_err(|err| AppError::msg(format!("无法连接 Sentinel: {err}")))?;
    let reply: Vec<String> = redis::cmd("SENTINEL")
        .arg("get-master-addr-by-name")
        .arg(service)
        .query(&mut conn)
        .map_err(|err| AppError::msg(format!("Sentinel 没有返回主节点: {err}")))?;
    if reply.len() < 2 {
        return Err(AppError::msg(format!("Sentinel 服务 {service} 没有主节点")));
    }
    let port: u16 = reply[1].parse().unwrap_or(6379);
    Ok(redis_url(&reply[0], port, &config.username, &config.password, 0))
}

impl RedisSession {
    fn select_db(&mut self, database: &str) -> AppResult<()> {
        if self.mode == "cluster" {
            return Ok(());
        }
        let db = database.trim().trim_start_matches("db").parse::<i64>().unwrap_or(0);
        redis::cmd("SELECT")
            .arg(db)
            .query::<()>(self.conn.as_mut())
            .map_err(|err| AppError::msg(err.to_string()))
    }
}

impl DbSession for RedisSession {
    fn engine(&self) -> Engine {
        Engine::Redis
    }

    fn is_document_store(&self) -> bool {
        true
    }

    fn version(&mut self) -> AppResult<String> {
        let info: String = redis::cmd("INFO").arg("server").query(self.conn.as_mut()).map_err(|err| AppError::msg(err.to_string()))?;
        let version = info
            .lines()
            .find_map(|line| line.strip_prefix("redis_version:"))
            .unwrap_or("redis")
            .to_string();
        Ok(format!("Redis {version} ({})", if self.mode.is_empty() { "standalone" } else { self.mode.as_str() }))
    }

    fn list_databases(&mut self) -> AppResult<Vec<String>> {
        if self.mode == "cluster" {
            return Ok(vec!["cluster".into()]);
        }
        let count = redis::cmd("CONFIG")
            .arg("GET")
            .arg("databases")
            .query::<redis::Value>(self.conn.as_mut())
            .ok()
            .and_then(|value| match value {
                redis::Value::Array(items) => items.get(1).and_then(redis_int),
                _ => None,
            })
            .unwrap_or(16)
            .clamp(1, 64);
        Ok((0..count).map(|index| index.to_string()).collect())
    }

    fn list_schemas(&mut self, database: &str) -> AppResult<Vec<String>> {
        let _ = database;
        Ok(vec!["keys".into()])
    }

    fn list_tables(&mut self, database: &str, _schema: &str) -> AppResult<Vec<String>> {
        self.select_db(database)?;
        let mut cursor = 0i64;
        let mut keys = Vec::new();
        for _ in 0..20 {
            let (next, batch): (i64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("COUNT")
                .arg(100)
                .query(self.conn.as_mut())
                .map_err(|err| AppError::msg(err.to_string()))?;
            keys.extend(batch);
            cursor = next;
            if cursor == 0 || keys.len() >= 300 {
                break;
            }
        }
        keys.sort();
        keys.dedup();
        keys.truncate(300);
        Ok(keys)
    }

    fn list_views(&mut self, _database: &str, _schema: &str) -> AppResult<Vec<String>> {
        Ok(Vec::new())
    }

    fn list_columns(&mut self, database: &str, _schema: &str, object_name: &str) -> AppResult<Vec<ColumnMeta>> {
        self.select_db(database)?;
        let kind = key_type(self.conn.as_mut(), object_name)?;
        Ok(columns_for(&kind))
    }

    fn relation_kind(&mut self, database: &str, _schema: &str, object_name: &str) -> AppResult<Option<String>> {
        self.select_db(database)?;
        let kind = key_type(self.conn.as_mut(), object_name)?;
        if kind == "none" {
            Ok(None)
        } else {
            Ok(Some(kind))
        }
    }

    fn execute(&mut self, database: &str, sql: &str, max_rows: Option<usize>) -> AppResult<QueryOutcome> {
        self.select_db(database)?;
        let line = sql.trim().trim_end_matches(';').trim();
        if line.is_empty() {
            return Err(AppError::msg("没有 Redis 命令"));
        }
        let mut parts = split_command(line);
        if parts.is_empty() {
            return Err(AppError::msg("没有 Redis 命令"));
        }
        let name = parts.remove(0).to_ascii_uppercase();
        let mut cmd = redis::cmd(&name);
        for part in parts {
            cmd.arg(part);
        }
        let value: redis::Value = cmd.query(self.conn.as_mut()).map_err(|err| AppError::msg(err.to_string()))?;
        Ok(value_to_outcome(value, max_rows.unwrap_or(500)))
    }

    fn browse_page(
        &mut self,
        database: &str,
        _schema: &str,
        table: &str,
        _filters: &[FilterSpec],
        _sorts: &[SortSpec],
        limit: u32,
        offset: u32,
    ) -> AppResult<QueryOutcome> {
        self.select_db(database)?;
        let kind = key_type(self.conn.as_mut(), table)?;
        let columns = columns_for(&kind);
        let mut rows = match kind.as_str() {
            "string" => {
                let value: Option<String> = redis::cmd("GET").arg(table).query(self.conn.as_mut()).map_err(|err| AppError::msg(err.to_string()))?;
                vec![vec![JsonValue::String(table.into()), JsonValue::String(value.unwrap_or_default())]]
            }
            "hash" => {
                let map: Vec<(String, String)> = redis::cmd("HGETALL").arg(table).query(self.conn.as_mut()).map_err(|err| AppError::msg(err.to_string()))?;
                map.into_iter().map(|(field, value)| vec![JsonValue::String(field), JsonValue::String(value)]).collect()
            }
            "list" => {
                let values: Vec<String> = redis::cmd("LRANGE").arg(table).arg(0).arg(499).query(self.conn.as_mut()).map_err(|err| AppError::msg(err.to_string()))?;
                values.into_iter().enumerate().map(|(index, value)| vec![json!(index as i64), JsonValue::String(value)]).collect()
            }
            "set" => {
                let values: Vec<String> = redis::cmd("SMEMBERS").arg(table).query(self.conn.as_mut()).map_err(|err| AppError::msg(err.to_string()))?;
                values.into_iter().map(|value| vec![JsonValue::String(value)]).collect()
            }
            "zset" => {
                let values: Vec<(String, f64)> = redis::cmd("ZRANGE").arg(table).arg(0).arg(499).arg("WITHSCORES").query(self.conn.as_mut()).map_err(|err| AppError::msg(err.to_string()))?;
                values.into_iter().map(|(member, score)| vec![JsonValue::String(member), json!(score)]).collect()
            }
            _ => return Err(AppError::msg(format!("键不存在或类型 {kind} 暂不能以表格打开"))),
        };
        let start = offset as usize;
        let end = (start + limit as usize).min(rows.len());
        let page = if start < rows.len() { rows.drain(start..end).collect() } else { Vec::new() };
        Ok(QueryOutcome {
            columns,
            rows: page,
            rows_affected: 0,
            truncated: false,
        })
    }

    fn count_relation(&mut self, database: &str, _schema: &str, table: &str) -> AppResult<u64> {
        self.select_db(database)?;
        let kind = key_type(self.conn.as_mut(), table)?;
        let count: i64 = match kind.as_str() {
            "string" => 1,
            "hash" => redis::cmd("HLEN").arg(table).query(self.conn.as_mut()).unwrap_or(0),
            "list" => redis::cmd("LLEN").arg(table).query(self.conn.as_mut()).unwrap_or(0),
            "set" => redis::cmd("SCARD").arg(table).query(self.conn.as_mut()).unwrap_or(0),
            "zset" => redis::cmd("ZCARD").arg(table).query(self.conn.as_mut()).unwrap_or(0),
            _ => 0,
        };
        Ok(count.max(0) as u64)
    }

    fn apply_mutation(&mut self, database: &str, _schema: &str, table: &str, change: &RowChange) -> AppResult<()> {
        self.select_db(database)?;
        let kind = key_type(self.conn.as_mut(), table).unwrap_or_else(|_| "none".into());
        match (kind.as_str(), change.kind.as_str()) {
            ("string", "update" | "insert") => {
                let value = change.values.iter().find(|item| item.column == "value").map(|item| json_text(&item.value)).unwrap_or_default();
                redis::cmd("SET").arg(table).arg(value).query::<()>(self.conn.as_mut()).map_err(|err| AppError::msg(err.to_string()))?;
            }
            ("hash", "insert" | "update") => {
                let field = field_of(change, "field")?;
                let value = change.values.iter().find(|item| item.column == "value").map(|item| json_text(&item.value)).unwrap_or_default();
                redis::cmd("HSET").arg(table).arg(field).arg(value).query::<()>(self.conn.as_mut()).map_err(|err| AppError::msg(err.to_string()))?;
            }
            ("hash", "delete") => {
                let field = field_of(change, "field")?;
                redis::cmd("HDEL").arg(table).arg(field).query::<()>(self.conn.as_mut()).map_err(|err| AppError::msg(err.to_string()))?;
            }
            ("list", "update") => {
                let index = field_of(change, "index")?;
                let value = change.values.iter().find(|item| item.column == "value").map(|item| json_text(&item.value)).unwrap_or_default();
                redis::cmd("LSET").arg(table).arg(index).arg(value).query::<()>(self.conn.as_mut()).map_err(|err| AppError::msg(err.to_string()))?;
            }
            ("list", "insert") => {
                let value = change.values.iter().find(|item| item.column == "value").map(|item| json_text(&item.value)).unwrap_or_default();
                redis::cmd("RPUSH").arg(table).arg(value).query::<()>(self.conn.as_mut()).map_err(|err| AppError::msg(err.to_string()))?;
            }
            ("set", "insert") => {
                let member = field_of(change, "member")?;
                redis::cmd("SADD").arg(table).arg(member).query::<()>(self.conn.as_mut()).map_err(|err| AppError::msg(err.to_string()))?;
            }
            ("set", "delete") => {
                let member = field_of(change, "member")?;
                redis::cmd("SREM").arg(table).arg(member).query::<()>(self.conn.as_mut()).map_err(|err| AppError::msg(err.to_string()))?;
            }
            ("zset", "insert" | "update") => {
                let member = field_of(change, "member")?;
                let score = change.values.iter().find(|item| item.column == "score").map(|item| json_text(&item.value)).unwrap_or_else(|| "0".into());
                redis::cmd("ZADD").arg(table).arg(score).arg(member).query::<()>(self.conn.as_mut()).map_err(|err| AppError::msg(err.to_string()))?;
            }
            ("zset", "delete") => {
                let member = field_of(change, "member")?;
                redis::cmd("ZREM").arg(table).arg(member).query::<()>(self.conn.as_mut()).map_err(|err| AppError::msg(err.to_string()))?;
            }
            ("none", "insert") if change.values.iter().any(|item| item.column == "value") => {
                let value = change.values.iter().find(|item| item.column == "value").map(|item| json_text(&item.value)).unwrap_or_default();
                redis::cmd("SET").arg(table).arg(value).query::<()>(self.conn.as_mut()).map_err(|err| AppError::msg(err.to_string()))?;
            }
            (_, "delete") => {
                redis::cmd("DEL").arg(table).query::<()>(self.conn.as_mut()).map_err(|err| AppError::msg(err.to_string()))?;
            }
            _ => return Err(AppError::msg(format!("不能对 {kind} 执行 {}", change.kind))),
        }
        Ok(())
    }
}

fn field_of(change: &RowChange, column: &str) -> AppResult<String> {
    change
        .pk
        .iter()
        .chain(change.values.iter())
        .find(|item| item.column == column)
        .map(|item| json_text(&item.value))
        .filter(|item| !item.is_empty())
        .ok_or_else(|| AppError::msg(format!("缺少 {column}")))
}

fn key_type(conn: &mut dyn redis::ConnectionLike, key: &str) -> AppResult<String> {
    redis::cmd("TYPE")
        .arg(key)
        .query(conn)
        .map_err(|err| AppError::msg(err.to_string()))
}

fn columns_for(kind: &str) -> Vec<ColumnMeta> {
    match kind {
        "string" => vec![pk("key", "string"), col("value", "string")],
        "hash" => vec![pk("field", "string"), col("value", "string")],
        "list" => vec![pk("index", "int"), col("value", "string")],
        "set" => vec![pk("member", "string")],
        "zset" => vec![pk("member", "string"), col("score", "float")],
        _ => vec![pk("key", "string"), col("value", "string")],
    }
}

fn pk(name: &str, data_type: &str) -> ColumnMeta {
    ColumnMeta {
        name: name.into(),
        data_type: data_type.into(),
        nullable: false,
        primary_key: true,
        auto_increment: false,
        default_value: None,
    }
}

fn col(name: &str, data_type: &str) -> ColumnMeta {
    ColumnMeta {
        name: name.into(),
        data_type: data_type.into(),
        nullable: true,
        primary_key: false,
        auto_increment: false,
        default_value: None,
    }
}

fn redis_int(value: &redis::Value) -> Option<i64> {
    match value {
        redis::Value::Int(value) => Some(*value),
        redis::Value::BulkString(bytes) => String::from_utf8_lossy(bytes).parse().ok(),
        redis::Value::SimpleString(text) => text.parse().ok(),
        _ => None,
    }
}

fn value_to_outcome(value: redis::Value, max_rows: usize) -> QueryOutcome {
    let mut rows = Vec::new();
    flatten("result", &value, &mut rows);
    let truncated = rows.len() > max_rows;
    rows.truncate(max_rows);
    let rows_affected = if rows.is_empty() { 1 } else { 0 };
    QueryOutcome {
        columns: vec![col("path", "string"), col("value", "string")],
        rows,
        rows_affected,
        truncated,
    }
}

fn flatten(path: &str, value: &redis::Value, rows: &mut Vec<Vec<JsonValue>>) {
    match value {
        redis::Value::Nil => rows.push(vec![JsonValue::String(path.into()), JsonValue::Null]),
        redis::Value::Int(value) => rows.push(vec![JsonValue::String(path.into()), json!(value)]),
        redis::Value::Okay => rows.push(vec![JsonValue::String(path.into()), JsonValue::String("OK".into())]),
        redis::Value::SimpleString(text) | redis::Value::VerbatimString { text, .. } => {
            rows.push(vec![JsonValue::String(path.into()), JsonValue::String(text.clone())])
        }
        redis::Value::BulkString(bytes) => rows.push(vec![JsonValue::String(path.into()), JsonValue::String(String::from_utf8_lossy(bytes).into_owned())]),
        redis::Value::Double(value) => rows.push(vec![JsonValue::String(path.into()), json!(value)]),
        redis::Value::Boolean(flag) => rows.push(vec![JsonValue::String(path.into()), JsonValue::Bool(*flag)]),
        redis::Value::Array(items) | redis::Value::Set(items) => {
            if items.is_empty() {
                rows.push(vec![JsonValue::String(path.into()), JsonValue::Array(vec![])]);
            }
            for (index, item) in items.iter().enumerate() {
                flatten(&format!("{path}[{index}]"), item, rows);
            }
        }
        redis::Value::Map(items) => {
            for (key, item) in items {
                flatten(&format!("{path}.{key:?}"), item, rows);
            }
        }
        other => rows.push(vec![JsonValue::String(path.into()), JsonValue::String(format!("{other:?}"))]),
    }
}

fn split_command(line: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for ch in line.chars() {
        match quote {
            Some(mark) if ch == mark => quote = None,
            Some(_) => current.push(ch),
            None if ch == '"' || ch == '\'' => quote = Some(ch),
            None if ch.is_whitespace() => {
                if !current.is_empty() {
                    parts.push(std::mem::take(&mut current));
                }
            }
            None => current.push(ch),
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

fn json_text(value: &JsonValue) -> String {
    match value {
        JsonValue::Null => String::new(),
        JsonValue::String(text) => text.clone(),
        JsonValue::Number(number) => number.to_string(),
        JsonValue::Bool(flag) => flag.to_string(),
        other => other.to_string(),
    }
}

impl MappedCluster {
    fn open(
        nodes: Vec<String>,
        map: HashMap<String, String>,
        username: &str,
        password: &str,
    ) -> AppResult<Self> {
        let mut links = Vec::new();
        for node in &nodes {
            let dial = node_hostport(node).unwrap_or_else(|| node.clone());
            let conn = dial_url(node)?;
            links.push(NodeLink { dial, conn });
        }
        if links.is_empty() {
            return Err(AppError::msg("Redis Cluster 需要至少一个节点"));
        }
        let mut cluster = Self {
            nodes: links,
            slots: Vec::new(),
            map,
            username: username.to_string(),
            password: password.to_string(),
        };
        cluster.load_slots()?;
        Ok(cluster)
    }

    fn load_slots(&mut self) -> AppResult<()> {
        let value: redis::Value = redis::cmd("CLUSTER")
            .arg("SLOTS")
            .query(&mut self.nodes[0].conn)
            .map_err(|err| AppError::msg(format!("CLUSTER SLOTS 失败: {err}")))?;
        let ranges = parse_cluster_slots(&value);
        if ranges.is_empty() {
            return Err(AppError::msg("CLUSTER SLOTS 没有返回槽位，地址映射无法建立"));
        }
        let mut slots = Vec::new();
        for (start, end, advertised) in ranges {
            let index = self.ensure_node_sync(&advertised)?;
            slots.push(SlotRange { start, end, index });
        }
        self.slots = slots;
        Ok(())
    }

    fn ensure_node_sync(&mut self, advertised: &str) -> AppResult<usize> {
        let dial = rewrite_address(advertised, &self.map);
        if let Some(index) = self.nodes.iter().position(|node| node.dial == dial) {
            return Ok(index);
        }
        let (host, port) = split_host_port(&dial, 6379);
        let url = redis_url(&host, port, &self.username, &self.password, 0);
        let conn = dial_url(&url)?;
        self.nodes.push(NodeLink { dial, conn });
        Ok(self.nodes.len() - 1)
    }

    fn ensure_node(&mut self, advertised: &str) -> redis::RedisResult<usize> {
        self.ensure_node_sync(advertised).map_err(|err| redis_err(err.to_string()))
    }

    fn route_index(&self, cmd: &[u8]) -> usize {
        let Some(key) = first_key(cmd) else {
            return 0;
        };
        let slot = key_slot(&key);
        self.slots
            .iter()
            .find(|range| slot >= range.start && slot <= range.end)
            .map(|range| range.index)
            .unwrap_or(0)
    }

    fn note_moved(&mut self, slot: u16, index: usize) {
        if let Some(range) = self.slots.iter_mut().find(|range| slot >= range.start && slot <= range.end) {
            range.index = index;
        } else {
            self.slots.push(SlotRange { start: slot, end: slot, index });
        }
    }

    fn send_to(&mut self, index: usize, cmd: &[u8], asking: bool) -> redis::RedisResult<redis::Value> {
        let index = index.min(self.nodes.len().saturating_sub(1));
        if asking {
            let _ = redis::cmd("ASKING").query::<redis::Value>(&mut self.nodes[index].conn);
        }
        redis::ConnectionLike::req_packed_command(&mut self.nodes[index].conn, cmd)
    }

    fn round(&mut self, cmd: &[u8]) -> redis::RedisResult<redis::Value> {
        if self.nodes.is_empty() {
            return Err(redis_err("没有可用的 Redis 节点".into()));
        }
        let index = self.route_index(cmd).min(self.nodes.len() - 1);
        match self.send_to(index, cmd, false) {
            Ok(value) => Ok(value),
            Err(err) => {
                let Some((kind, slot, addr)) = redirect_from(&err.to_string()) else {
                    return Err(err);
                };
                let index = self.ensure_node(&addr)?;
                if matches!(kind, RedirectKind::Moved) {
                    self.note_moved(slot, index);
                }
                self.send_to(index, cmd, matches!(kind, RedirectKind::Ask))
            }
        }
    }
}

impl redis::ConnectionLike for MappedCluster {
    fn req_packed_command(&mut self, cmd: &[u8]) -> redis::RedisResult<redis::Value> {
        self.round(cmd)
    }

    fn req_packed_commands(&mut self, cmd: &[u8], offset: usize, count: usize) -> redis::RedisResult<Vec<redis::Value>> {
        let parts = split_resp_commands(cmd);
        let commands = if parts.is_empty() { vec![cmd] } else { parts };
        let mut values = Vec::with_capacity(commands.len());
        for command in commands {
            values.push(self.req_packed_command(command)?);
        }
        let end = (offset + count).min(values.len());
        if offset >= values.len() {
            Ok(Vec::new())
        } else {
            Ok(values[offset..end].to_vec())
        }
    }

    fn get_db(&self) -> i64 {
        0
    }

    fn check_connection(&mut self) -> bool {
        self.nodes.first_mut().is_some_and(|node| {
            redis::cmd("PING").query::<String>(&mut node.conn).is_ok()
        })
    }

    fn is_open(&self) -> bool {
        self.nodes.iter().any(|node| node.conn.is_open())
    }
}

pub fn merge_address_map(user: &str, auto: &str) -> String {
    match (user.trim().is_empty(), auto.trim().is_empty()) {
        (true, true) => String::new(),
        (true, false) => auto.trim().to_string(),
        (false, true) => user.trim().to_string(),
        (false, false) => format!("{},{}", user.trim(), auto.trim()),
    }
}

pub fn parse_address_map(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for part in text.split([',', ';', '\n']) {
        let Some((from, to)) = part.split_once('=') else {
            continue;
        };
        let from = normalize_addr(from);
        let to = normalize_addr(to);
        if !from.is_empty() && !to.is_empty() {
            map.insert(from, to);
        }
    }
    map
}

pub fn rewrite_address(addr: &str, map: &HashMap<String, String>) -> String {
    let key = normalize_addr(addr);
    map.get(&key).cloned().unwrap_or(key)
}

fn normalize_addr(addr: &str) -> String {
    let addr = addr.trim();
    if addr.is_empty() {
        return String::new();
    }
    let (host, port) = split_host_port(addr, 6379);
    format!("{}:{port}", host.to_ascii_lowercase())
}

pub fn key_slot(key: &[u8]) -> u16 {
    crc16(hash_tag(key)) % 16384
}

fn hash_tag(key: &[u8]) -> &[u8] {
    if let Some(start) = key.iter().position(|byte| *byte == b'{') {
        if let Some(end) = key[start + 1..].iter().position(|byte| *byte == b'}') {
            if end > 0 {
                return &key[start + 1..start + 1 + end];
            }
        }
    }
    key
}

fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &byte in data {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            if crc & 0x8000 != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

pub fn first_key(buf: &[u8]) -> Option<Vec<u8>> {
    let args = parse_resp_args(buf)?;
    if args.is_empty() {
        return None;
    }
    let name = String::from_utf8_lossy(&args[0]).to_ascii_uppercase();
    const KEYLESS: &[&str] = &[
        "PING", "INFO", "AUTH", "SELECT", "CLUSTER", "CONFIG", "DBSIZE", "FLUSHALL", "FLUSHDB", "SLOWLOG",
        "CLIENT", "COMMAND", "ECHO", "TIME", "ROLE", "REPLICAOF", "SLAVEOF", "SHUTDOWN", "SCRIPT", "MEMORY",
        "LATENCY", "MODULE", "ACL", "HELLO", "RESET", "WAIT", "DEBUG", "MONITOR", "SYNC", "PSYNC", "ASKING",
    ];
    if KEYLESS.contains(&name.as_str()) {
        return None;
    }
    if matches!(name.as_str(), "EVAL" | "EVALSHA" | "EVAL_RO" | "FCALL" | "FCALL_RO") {
        let count = args.get(2).and_then(|bytes| std::str::from_utf8(bytes).ok()).and_then(|text| text.parse::<usize>().ok()).unwrap_or(0);
        return if count == 0 { None } else { args.get(3).cloned() };
    }
    args.get(1).cloned()
}

fn parse_resp_args(buf: &[u8]) -> Option<Vec<Vec<u8>>> {
    if buf.first() != Some(&b'*') {
        return None;
    }
    let (argc, mut index) = read_line_number(&buf[1..])?;
    index += 1;
    let mut args = Vec::with_capacity(argc);
    for _ in 0..argc {
        if buf.get(index) != Some(&b'$') {
            return None;
        }
        let (len, next) = read_line_number(&buf[index + 1..])?;
        index += 1 + next;
        let end = index + len;
        if end + 2 > buf.len() {
            return None;
        }
        args.push(buf[index..end].to_vec());
        index = end + 2;
    }
    Some(args)
}

fn split_resp_commands(buf: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut index = 0;
    while index < buf.len() {
        let start = index;
        if buf[index] != b'*' {
            break;
        }
        let Some((argc, next)) = read_line_number(&buf[index + 1..]) else {
            break;
        };
        index += 1 + next;
        let mut ok = true;
        for _ in 0..argc {
            if buf.get(index) != Some(&b'$') {
                ok = false;
                break;
            }
            let Some((len, next)) = read_line_number(&buf[index + 1..]) else {
                ok = false;
                break;
            };
            index += 1 + next + len + 2;
            if index > buf.len() {
                ok = false;
                break;
            }
        }
        if !ok {
            break;
        }
        out.push(&buf[start..index]);
    }
    out
}

fn read_line_number(buf: &[u8]) -> Option<(usize, usize)> {
    let end = buf.iter().position(|byte| *byte == b'\r')?;
    let text = std::str::from_utf8(&buf[..end]).ok()?;
    let number = text.parse::<usize>().ok()?;
    Some((number, end + 2))
}

pub fn parse_cluster_slots(value: &redis::Value) -> Vec<(u16, u16, String)> {
    let redis::Value::Array(ranges) = value else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for range in ranges {
        let redis::Value::Array(items) = range else {
            continue;
        };
        let Some(start) = items.first().and_then(value_u16) else {
            continue;
        };
        let Some(end) = items.get(1).and_then(value_u16) else {
            continue;
        };
        let Some(redis::Value::Array(primary)) = items.get(2) else {
            continue;
        };
        let Some(host) = primary.first().and_then(value_text) else {
            continue;
        };
        let port = primary.get(1).and_then(value_u16).unwrap_or(6379);
        out.push((start, end, format!("{host}:{port}")));
    }
    out
}

fn value_u16(value: &redis::Value) -> Option<u16> {
    match value {
        redis::Value::Int(number) if *number >= 0 && *number <= u16::MAX as i64 => Some(*number as u16),
        redis::Value::BulkString(bytes) => String::from_utf8_lossy(bytes).parse().ok(),
        _ => None,
    }
}

fn value_text(value: &redis::Value) -> Option<String> {
    match value {
        redis::Value::BulkString(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
        redis::Value::SimpleString(text) => Some(text.clone()),
        _ => None,
    }
}

enum RedirectKind {
    Moved,
    Ask,
}

fn redirect_from(text: &str) -> Option<(RedirectKind, u16, String)> {
    let upper = text.to_ascii_uppercase();
    let (kind, token) = if let Some(pos) = upper.find("MOVED ") {
        (RedirectKind::Moved, pos + "MOVED ".len())
    } else if let Some(pos) = upper.find("ASK ") {
        (RedirectKind::Ask, pos + "ASK ".len())
    } else {
        return None;
    };
    let mut parts = text[token..].split_whitespace();
    let slot = parts.next()?.trim_matches(',').parse().ok()?;
    let addr = parts.next()?.trim_matches(|ch: char| matches!(ch, ',' | '\'' | '"' | ')'));
    if addr.is_empty() {
        None
    } else {
        Some((kind, slot, addr.to_string()))
    }
}

fn node_hostport(url: &str) -> Option<String> {
    let hostport = if let Some((_, rest)) = url.split_once("://") {
        rest.split('@').next_back()?.split('/').next()?
    } else {
        url
    };
    let (host, port) = split_host_port(hostport, 6379);
    Some(format!("{}:{port}", host.to_ascii_lowercase()))
}

fn dial_url(url: &str) -> AppResult<redis::Connection> {
    let client = redis::Client::open(url).map_err(|err| AppError::msg(err.to_string()))?;
    let conn = client.get_connection().map_err(|err| AppError::msg(err.to_string()))?;
    conn.set_read_timeout(Some(Duration::from_secs(15))).ok();
    conn.set_write_timeout(Some(Duration::from_secs(15))).ok();
    Ok(conn)
}

fn redis_err(message: String) -> redis::RedisError {
    redis::RedisError::from((redis::ErrorKind::IoError, "dataharbor", message))
}

pub fn publish(config: &SessionConfig, channel: &str, message: &str) -> AppResult<i64> {
    let mut session = connect(config)?;
    let receivers: i64 = redis::cmd("PUBLISH")
        .arg(channel)
        .arg(message)
        .query(session.conn.as_mut())
        .map_err(|err| AppError::msg(err.to_string()))?;
    Ok(receivers)
}

pub fn read_pubsub(config: &SessionConfig, channel: &str, wait_ms: u64) -> AppResult<Vec<String>> {
    let client = redis::Client::open(redis_url(&config.host, config.port, &config.username, &config.password, 0))
        .map_err(|err| AppError::msg(err.to_string()))?;
    let mut conn = client.get_connection().map_err(|err| AppError::msg(err.to_string()))?;
    let mut pubsub = conn.as_pubsub();
    pubsub.subscribe(channel).map_err(|err| AppError::msg(err.to_string()))?;
    pubsub.set_read_timeout(Some(Duration::from_millis(wait_ms.clamp(200, 10_000)))).ok();
    let mut messages = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_millis(wait_ms.clamp(200, 10_000));
    while std::time::Instant::now() < deadline && messages.len() < 50 {
        match pubsub.get_message() {
            Ok(message) => {
                let payload: String = message.get_payload().unwrap_or_default();
                messages.push(payload);
            }
            Err(_) => break,
        }
    }
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_map_rewrites_advertised_nodes() {
        let merged = merge_address_map("10.0.0.9:6379=127.0.0.1:4601", "10.0.0.8:6379=127.0.0.1:4600");
        let map = parse_address_map(&merged);
        assert_eq!(rewrite_address("10.0.0.8:6379", &map), "127.0.0.1:4600");
        assert_eq!(rewrite_address("10.0.0.9:6379", &map), "127.0.0.1:4601");
        assert_eq!(rewrite_address("10.0.0.7:6379", &map), "10.0.0.7:6379");
        let auto_wins = parse_address_map(&merge_address_map(
            "10.0.0.8:6379=10.9.9.9:1",
            "10.0.0.8:6379=127.0.0.1:4600",
        ));
        assert_eq!(rewrite_address("10.0.0.8:6379", &auto_wins), "127.0.0.1:4600");
    }

    #[test]
    fn cluster_slot_and_packed_key() {
        assert_eq!(crc16(b"123456789"), 0x31C3);
        assert_eq!(key_slot(b"123456789"), 12739);
        assert_eq!(key_slot(b"{user}.foo"), key_slot(b"user"));
        let packed = redis::cmd("GET").arg("foo").get_packed_command();
        assert_eq!(first_key(&packed).as_deref(), Some(b"foo".as_slice()));
        let eval = redis::cmd("EVAL").arg("return 1").arg(1).arg("foo").get_packed_command();
        assert_eq!(first_key(&eval).as_deref(), Some(b"foo".as_slice()));
        let ping = redis::cmd("PING").get_packed_command();
        assert!(first_key(&ping).is_none());
    }

    #[test]
    fn cluster_slots_primary_can_be_rewritten() {
        let value = redis::Value::Array(vec![redis::Value::Array(vec![
            redis::Value::Int(0),
            redis::Value::Int(5460),
            redis::Value::Array(vec![
                redis::Value::BulkString(b"10.0.0.8".to_vec()),
                redis::Value::Int(6379),
                redis::Value::BulkString(b"node".to_vec()),
            ]),
        ])]);
        let slots = parse_cluster_slots(&value);
        assert_eq!(slots, vec![(0, 5460, "10.0.0.8:6379".into())]);
        let map = parse_address_map("10.0.0.8:6379=127.0.0.1:4600");
        assert_eq!(rewrite_address(&slots[0].2, &map), "127.0.0.1:4600");
        let (kind, slot, addr) = redirect_from("MOVED 3999 10.0.0.8:6379").unwrap();
        assert!(matches!(kind, RedirectKind::Moved));
        assert_eq!(slot, 3999);
        assert_eq!(rewrite_address(&addr, &map), "127.0.0.1:4600");
    }
}
