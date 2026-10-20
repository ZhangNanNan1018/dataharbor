use std::time::Duration;

use futures::TryStreamExt;
use mongodb::bson::{doc, Bson, Document};
use mongodb::options::{ClientOptions, Credential, FindOptions, Tls, TlsOptions};
use mongodb::Client;
use serde_json::{Map, Number, Value as JsonValue};

use super::runtime::block_on;
use super::DbSession;
use crate::error::{AppError, AppResult};
use crate::models::{ColumnMeta, Engine, FilterSpec, QueryOutcome, RowChange, SessionConfig, SortSpec};

pub struct MongoSession {
    client: Client,
}

pub fn connect(config: &SessionConfig) -> AppResult<MongoSession> {
    if config.use_ssl && config.tls_server_name.is_some() && !config.ssl_insecure {
        return Err(AppError::msg("当前 MongoDB 驱动不能为隧道单独指定 TLS 证书主机名，已拒绝 SSL 隧道连接；请使用直接 TLS 连接。"));
    }
    block_on(async {
        let host = config.host.trim();
        let port = if config.port == 0 { 27017 } else { config.port };
        let uri = if host.starts_with("mongodb://") || host.starts_with("mongodb+srv://") {
            host.to_string()
        } else {
            format!("mongodb://{host}:{port}")
        };
        let mut options = ClientOptions::parse(uri).await.map_err(|err| AppError::msg(format!("MongoDB 地址无效: {err}")))?;
        if config.use_ssl {
            let tls = TlsOptions::builder()
                .allow_invalid_certificates(config.ssl_insecure)
                .build();
            options.tls = Some(Tls::Enabled(tls));
        }
        options.connect_timeout = Some(Duration::from_secs(8));
        options.server_selection_timeout = Some(Duration::from_secs(8));
        if !config.username.trim().is_empty() {
            options.credential = Some(
                Credential::builder()
                    .username(config.username.trim().to_string())
                    .password(config.password.clone())
                    .build(),
            );
        }
        if !config.database.trim().is_empty() {
            options.default_database = Some(config.database.trim().to_string());
        }
        let client = Client::with_options(options).map_err(|err| AppError::msg(err.to_string()))?;
        client
            .database("admin")
            .run_command(doc! { "ping": 1 }, None)
            .await
            .map_err(|err| AppError::msg(format!("MongoDB 连接失败: {err}")))?;
        Ok(MongoSession { client })
    })
}

impl DbSession for MongoSession {
    fn engine(&self) -> Engine {
        Engine::Mongodb
    }

    fn is_document_store(&self) -> bool {
        true
    }

    fn version(&mut self) -> AppResult<String> {
        block_on(async {
            let doc = self
                .client
                .database("admin")
                .run_command(doc! { "buildInfo": 1 }, None)
                .await
                .map_err(|err| AppError::msg(err.to_string()))?;
            let version = doc.get_str("version").unwrap_or("unknown");
            Ok(format!("MongoDB {version}"))
        })
    }

    fn list_databases(&mut self) -> AppResult<Vec<String>> {
        block_on(async {
            self.client
                .list_database_names(None, None)
                .await
                .map_err(|err| AppError::msg(err.to_string()))
        })
    }

    fn list_schemas(&mut self, database: &str) -> AppResult<Vec<String>> {
        Ok(vec![if database.is_empty() { "default".into() } else { database.to_string() }])
    }

    fn list_tables(&mut self, database: &str, _schema: &str) -> AppResult<Vec<String>> {
        block_on(async {
            self.client
                .database(database)
                .list_collection_names(None)
                .await
                .map_err(|err| AppError::msg(err.to_string()))
        })
    }

    fn list_views(&mut self, _database: &str, _schema: &str) -> AppResult<Vec<String>> {
        Ok(Vec::new())
    }

    fn list_columns(&mut self, database: &str, _schema: &str, object_name: &str) -> AppResult<Vec<ColumnMeta>> {
        block_on(async {
            let mut cursor = self
                .client
                .database(database)
                .collection::<Document>(object_name)
                .find(doc! {}, FindOptions::builder().limit(1).build())
                .await
                .map_err(|err| AppError::msg(err.to_string()))?;
            let doc = cursor.try_next().await.map_err(|err| AppError::msg(err.to_string()))?.unwrap_or_default();
            let mut columns = Vec::new();
            if !doc.contains_key("_id") {
                columns.push(pk_col("_id"));
            }
            for (key, value) in doc.iter() {
                columns.push(ColumnMeta {
                    name: key.clone(),
                    data_type: bson_type_name(value).into(),
                    nullable: key != "_id",
                    primary_key: key == "_id",
                    auto_increment: false,
                    default_value: None,
                });
            }
            if columns.is_empty() {
                columns.push(pk_col("_id"));
            }
            Ok(columns)
        })
    }

    fn relation_kind(&mut self, database: &str, _schema: &str, object_name: &str) -> AppResult<Option<String>> {
        let tables = self.list_tables(database, "")?;
        if tables.iter().any(|name| name == object_name) {
            Ok(Some("collection".into()))
        } else {
            Ok(None)
        }
    }

    fn execute(&mut self, database: &str, sql: &str, max_rows: Option<usize>) -> AppResult<QueryOutcome> {
        let text = sql.trim().trim_end_matches(';').trim();
        if let Some(rest) = text.strip_prefix("aggregate ") {
            let (name, pipeline) = rest.split_once(' ').unwrap_or((rest, "[]"));
            return self.aggregate(database, name.trim(), pipeline);
        }
        if let Some(rest) = text.strip_prefix("find ") {
            let mut parts = rest.splitn(2, ' ');
            let name = parts.next().unwrap_or("").trim();
            let filter = parts.next().unwrap_or("{}");
            return self.find_json(database, name, filter, max_rows.unwrap_or(100) as i64);
        }
        if text.starts_with('{') {
            return block_on(async {
                let command: Document = serde_json::from_str::<serde_json::Value>(text)
                    .ok()
                    .and_then(|value| bson_from_json(&value).as_document().cloned())
                    .ok_or_else(|| AppError::msg("命令需要是 JSON 对象"))?;
                let doc = self
                    .client
                    .database(if database.is_empty() { "admin" } else { database })
                    .run_command(command, None)
                    .await
                    .map_err(|err| AppError::msg(err.to_string()))?;
                Ok(document_outcome(doc))
            });
        }
        Err(AppError::msg("MongoDB 查询支持 find <集合> {过滤}、aggregate <集合> [管道]，或一条 JSON 命令"))
    }

    fn browse_page(
        &mut self,
        database: &str,
        _schema: &str,
        table: &str,
        filters: &[FilterSpec],
        sorts: &[SortSpec],
        limit: u32,
        offset: u32,
    ) -> AppResult<QueryOutcome> {
        let filter = filters_to_doc(filters);
        let mut sort = Document::new();
        for item in sorts.iter().take(3) {
            sort.insert(item.column.clone(), if item.desc { -1 } else { 1 });
        }
        block_on(async {
            let options = FindOptions::builder()
                .skip(Some(offset as u64))
                .limit(Some(limit as i64))
                .sort(if sort.is_empty() { None } else { Some(sort) })
                .build();
            let mut cursor = self
                .client
                .database(database)
                .collection::<Document>(table)
                .find(filter, options)
                .await
                .map_err(|err| AppError::msg(err.to_string()))?;
            let mut docs = Vec::new();
            while let Some(doc) = cursor.try_next().await.map_err(|err| AppError::msg(err.to_string()))? {
                docs.push(doc);
            }
            Ok(docs_outcome(docs))
        })
    }

    fn count_relation(&mut self, database: &str, _schema: &str, table: &str) -> AppResult<u64> {
        block_on(async {
            self.client
                .database(database)
                .collection::<Document>(table)
                .count_documents(doc! {}, None)
                .await
                .map_err(|err| AppError::msg(err.to_string()))
        })
    }

    fn apply_mutation(&mut self, database: &str, _schema: &str, table: &str, change: &RowChange) -> AppResult<()> {
        block_on(async {
            let coll = self.client.database(database).collection::<Document>(table);
            match change.kind.as_str() {
                "insert" => {
                    let mut doc = Document::new();
                    for field in &change.values {
                        if field.column == "_id" && field.value.is_null() {
                            continue;
                        }
                        doc.insert(field.column.clone(), bson_from_json(&field.value));
                    }
                    coll.insert_one(doc, None).await.map_err(|err| AppError::msg(err.to_string()))?;
                }
                "update" => {
                    let id = id_filter(change)?;
                    let mut set = Document::new();
                    for field in &change.values {
                        if field.column == "_id" {
                            continue;
                        }
                        set.insert(field.column.clone(), bson_from_json(&field.value));
                    }
                    if !set.is_empty() {
                        coll.update_one(id, doc! { "$set": set }, None)
                            .await
                            .map_err(|err| AppError::msg(err.to_string()))?;
                    }
                }
                "delete" => {
                    coll.delete_one(id_filter(change)?, None)
                        .await
                        .map_err(|err| AppError::msg(err.to_string()))?;
                }
                _ => return Err(AppError::msg("未知的更改类型")),
            }
            Ok(())
        })
    }
}

impl MongoSession {
    fn find_json(&self, database: &str, collection: &str, filter: &str, limit: i64) -> AppResult<QueryOutcome> {
        let filter_value: JsonValue = serde_json::from_str(filter).unwrap_or_else(|_| json_obj());
        let filter = match bson_from_json(&filter_value) {
            Bson::Document(doc) => doc,
            _ => Document::new(),
        };
        block_on(async {
            let options = FindOptions::builder().limit(Some(limit)).build();
            let mut cursor = self
                .client
                .database(database)
                .collection::<Document>(collection)
                .find(filter, options)
                .await
                .map_err(|err| AppError::msg(err.to_string()))?;
            let mut docs = Vec::new();
            while let Some(doc) = cursor.try_next().await.map_err(|err| AppError::msg(err.to_string()))? {
                docs.push(doc);
            }
            Ok(docs_outcome(docs))
        })
    }

    fn aggregate(&self, database: &str, collection: &str, pipeline_json: &str) -> AppResult<QueryOutcome> {
        let value: JsonValue = serde_json::from_str(pipeline_json).map_err(|err| AppError::msg(format!("管道不是 JSON: {err}")))?;
        let Bson::Array(stages) = bson_from_json(&value) else {
            return Err(AppError::msg("聚合管道需要是 JSON 数组"));
        };
        let pipeline = stages
            .into_iter()
            .filter_map(|item| match item {
                Bson::Document(doc) => Some(doc),
                _ => None,
            })
            .collect::<Vec<_>>();
        block_on(async {
            let mut cursor = self
                .client
                .database(database)
                .collection::<Document>(collection)
                .aggregate(pipeline, None)
                .await
                .map_err(|err| AppError::msg(err.to_string()))?;
            let mut docs = Vec::new();
            while let Some(doc) = cursor.try_next().await.map_err(|err| AppError::msg(err.to_string()))? {
                docs.push(doc);
            }
            Ok(docs_outcome(docs))
        })
    }
}

fn json_obj() -> JsonValue {
    JsonValue::Object(Map::new())
}

fn pk_col(name: &str) -> ColumnMeta {
    ColumnMeta {
        name: name.into(),
        data_type: "objectId".into(),
        nullable: false,
        primary_key: true,
        auto_increment: false,
        default_value: None,
    }
}

fn bson_type_name(value: &Bson) -> &'static str {
    match value {
        Bson::ObjectId(_) => "objectId",
        Bson::String(_) => "string",
        Bson::Int32(_) | Bson::Int64(_) => "int",
        Bson::Double(_) | Bson::Decimal128(_) => "double",
        Bson::Boolean(_) => "bool",
        Bson::DateTime(_) => "date",
        Bson::Array(_) => "array",
        Bson::Document(_) => "object",
        Bson::Null => "null",
        _ => "bson",
    }
}

fn docs_outcome(docs: Vec<Document>) -> QueryOutcome {
    let mut names = Vec::new();
    for doc in &docs {
        for key in doc.keys() {
            if !names.iter().any(|name| name == key) {
                names.push(key.clone());
            }
        }
    }
    if names.is_empty() {
        names.push("_id".into());
    }
    let columns = names
        .iter()
        .map(|name| ColumnMeta {
            name: name.clone(),
            data_type: "bson".into(),
            nullable: name != "_id",
            primary_key: name == "_id",
            auto_increment: false,
            default_value: None,
        })
        .collect::<Vec<_>>();
    let rows = docs
        .into_iter()
        .map(|doc| {
            names
                .iter()
                .map(|name| doc.get(name).map(json_from_bson).unwrap_or(JsonValue::Null))
                .collect()
        })
        .collect();
    QueryOutcome {
        columns,
        rows,
        rows_affected: 0,
        truncated: false,
    }
}

fn document_outcome(doc: Document) -> QueryOutcome {
    docs_outcome(vec![doc])
}

fn filters_to_doc(filters: &[FilterSpec]) -> Document {
    let mut doc = Document::new();
    for filter in filters {
        let value = match filter.value.as_deref() {
            Some(text) if !text.is_empty() => Bson::String(text.to_string()),
            _ => Bson::Null,
        };
        let clause = match filter.op.as_str() {
            "eq" => value,
            "ne" => Bson::Document(doc! { "$ne": value }),
            "gt" => Bson::Document(doc! { "$gt": value }),
            "lt" => Bson::Document(doc! { "$lt": value }),
            "gte" => Bson::Document(doc! { "$gte": value }),
            "lte" => Bson::Document(doc! { "$lte": value }),
            "contains" => Bson::Document(doc! { "$regex": filter.value.clone().unwrap_or_default(), "$options": "i" }),
            "is_null" => Bson::Null,
            "is_not_null" => Bson::Document(doc! { "$ne": Bson::Null }),
            _ => value,
        };
        doc.insert(filter.column.clone(), clause);
    }
    doc
}

fn id_filter(change: &RowChange) -> AppResult<Document> {
    let field = change
        .pk
        .iter()
        .find(|item| item.column == "_id")
        .ok_or_else(|| AppError::msg("MongoDB 更新和删除需要 _id"))?;
    Ok(doc! { "_id": bson_from_json(&field.value) })
}

fn json_from_bson(value: &Bson) -> JsonValue {
    match value {
        Bson::Null => JsonValue::Null,
        Bson::Boolean(flag) => JsonValue::Bool(*flag),
        Bson::Int32(value) => JsonValue::Number((*value).into()),
        Bson::Int64(value) => serde_json::Number::from_i128(i128::from(*value))
            .map(JsonValue::Number)
            .unwrap_or_else(|| JsonValue::String(value.to_string())),
        Bson::Double(value) => Number::from_f64(*value).map(JsonValue::Number).unwrap_or(JsonValue::Null),
        Bson::String(text) => JsonValue::String(text.clone()),
        Bson::ObjectId(id) => JsonValue::String(format!("oid:{id}")),
        Bson::DateTime(time) => JsonValue::String(time.try_to_rfc3339_string().unwrap_or_else(|_| time.timestamp_millis().to_string())),
        Bson::Array(items) => JsonValue::Array(items.iter().map(json_from_bson).collect()),
        Bson::Document(doc) => {
            let mut map = Map::new();
            for (key, value) in doc.iter() {
                map.insert(key.clone(), json_from_bson(value));
            }
            JsonValue::Object(map)
        }
        other => JsonValue::String(other.to_string()),
    }
}

fn bson_from_json(value: &JsonValue) -> Bson {
    match value {
        JsonValue::Null => Bson::Null,
        JsonValue::Bool(flag) => Bson::Boolean(*flag),
        JsonValue::Number(number) => {
            if let Some(value) = number.as_i64() {
                Bson::Int64(value)
            } else if let Some(value) = number.as_f64() {
                Bson::Double(value)
            } else {
                Bson::String(number.to_string())
            }
        }
        JsonValue::String(text) => {
            if let Some(hex) = text.strip_prefix("oid:") {
                if let Ok(id) = hex.parse::<mongodb::bson::oid::ObjectId>() {
                    return Bson::ObjectId(id);
                }
            }
            Bson::String(text.clone())
        }
        JsonValue::Array(items) => Bson::Array(items.iter().map(bson_from_json).collect()),
        JsonValue::Object(map) => {
            let mut doc = Document::new();
            for (key, value) in map {
                doc.insert(key.clone(), bson_from_json(value));
            }
            Bson::Document(doc)
        }
    }
}
