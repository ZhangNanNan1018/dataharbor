use std::time::Duration;

use futures::TryStreamExt;
use mongodb::bson::{doc, Bson, Document};
use mongodb::options::{ClientOptions, Credential, FindOptions, Tls, TlsOptions};
use mongodb::Client;
use serde_json::Value as JsonValue;

use super::runtime::block_on;
use super::DbSession;
use crate::error::{AppError, AppResult};
use crate::models::{
    ColumnMeta, Engine, FieldValue, FilterSpec, QueryOutcome, RowChange, SessionConfig, SortSpec,
};

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
        let mut options = ClientOptions::parse(uri)
            .await
            .map_err(|err| AppError::msg(format!("MongoDB 地址无效: {err}")))?;
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
        Ok(vec![if database.is_empty() {
            "default".into()
        } else {
            database.to_string()
        }])
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

    fn list_columns(
        &mut self,
        database: &str,
        _schema: &str,
        object_name: &str,
    ) -> AppResult<Vec<ColumnMeta>> {
        block_on(async {
            let mut cursor = self
                .client
                .database(database)
                .collection::<Document>(object_name)
                .find(doc! {}, FindOptions::builder().limit(1).build())
                .await
                .map_err(|err| AppError::msg(err.to_string()))?;
            let doc = cursor
                .try_next()
                .await
                .map_err(|err| AppError::msg(err.to_string()))?
                .unwrap_or_default();
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

    fn relation_kind(
        &mut self,
        database: &str,
        _schema: &str,
        object_name: &str,
    ) -> AppResult<Option<String>> {
        let tables = self.list_tables(database, "")?;
        if tables.iter().any(|name| name == object_name) {
            Ok(Some("collection".into()))
        } else {
            Ok(None)
        }
    }

    fn execute(
        &mut self,
        database: &str,
        sql: &str,
        max_rows: Option<usize>,
    ) -> AppResult<QueryOutcome> {
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
                let value: JsonValue = serde_json::from_str(text)
                    .map_err(|err| AppError::msg(format!("命令不是 JSON：{err}")))?;
                let command = bson_from_json(&value)?
                    .as_document()
                    .cloned()
                    .ok_or_else(|| AppError::msg("命令需要是 JSON 对象"))?;
                let doc = self
                    .client
                    .database(if database.is_empty() {
                        "admin"
                    } else {
                        database
                    })
                    .run_command(command, None)
                    .await
                    .map_err(|err| AppError::msg(err.to_string()))?;
                Ok(document_outcome(doc))
            });
        }
        Err(AppError::msg(
            "MongoDB 查询支持 find <集合> {过滤}、aggregate <集合> [管道]，或一条 JSON 命令",
        ))
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
        let filter = filters_to_doc(filters)?;
        let sort = stable_sort(sorts)?;
        block_on(async {
            let options = FindOptions::builder()
                .skip(Some(offset as u64))
                .limit(Some(limit as i64))
                .sort(Some(sort))
                .build();
            let mut cursor = self
                .client
                .database(database)
                .collection::<Document>(table)
                .find(filter, options)
                .await
                .map_err(|err| AppError::msg(err.to_string()))?;
            let mut docs = Vec::new();
            while let Some(doc) = cursor
                .try_next()
                .await
                .map_err(|err| AppError::msg(err.to_string()))?
            {
                docs.push(doc);
            }
            Ok(docs_outcome(docs))
        })
    }

    fn count_relation(&mut self, database: &str, schema: &str, table: &str) -> AppResult<u64> {
        self.count_filtered_relation(database, schema, table, &[])
    }

    fn count_filtered_relation(
        &mut self,
        database: &str,
        _schema: &str,
        table: &str,
        filters: &[FilterSpec],
    ) -> AppResult<u64> {
        let filter = filters_to_doc(filters)?;
        block_on(async {
            self.client
                .database(database)
                .collection::<Document>(table)
                .count_documents(filter, None)
                .await
                .map_err(|err| AppError::msg(err.to_string()))
        })
    }

    fn apply_mutation(
        &mut self,
        database: &str,
        _schema: &str,
        table: &str,
        change: &RowChange,
    ) -> AppResult<()> {
        block_on(async {
            let coll = self.client.database(database).collection::<Document>(table);
            match change.kind.as_str() {
                "insert" => {
                    let mut doc = Document::new();
                    for field in &change.values {
                        if field.column == "_id" && field.value.is_null() {
                            continue;
                        }
                        validate_field(&field.column)?;
                        doc.insert(field.column.clone(), bson_from_json(&field.value)?);
                    }
                    coll.insert_one(doc, None)
                        .await
                        .map_err(|err| AppError::msg(err.to_string()))?;
                }
                "update" => {
                    let id = original_filter(change)?;
                    let mut set = Document::new();
                    for field in &change.values {
                        if field.column == "_id" {
                            if !change
                                .pk
                                .iter()
                                .any(|pk| pk.column == "_id" && pk.value == field.value)
                            {
                                return Err(AppError::msg("MongoDB _id 不可修改"));
                            }
                            continue;
                        }
                        validate_field(&field.column)?;
                        set.insert(field.column.clone(), bson_from_json(&field.value)?);
                    }
                    if !set.is_empty() {
                        let result = coll
                            .update_one(id, doc! { "$set": set }, None)
                            .await
                            .map_err(|err| AppError::msg(err.to_string()))?;
                        if result.matched_count != 1 {
                            return Err(conflict());
                        }
                    } else {
                        return Err(AppError::msg("没有可更新的字段"));
                    }
                }
                "delete" => {
                    let result = coll
                        .delete_one(original_filter(change)?, None)
                        .await
                        .map_err(|err| AppError::msg(err.to_string()))?;
                    if result.deleted_count != 1 {
                        return Err(conflict());
                    }
                }
                _ => return Err(AppError::msg("未知的更改类型")),
            }
            Ok(())
        })
    }
}

impl MongoSession {
    fn find_json(
        &self,
        database: &str,
        collection: &str,
        filter: &str,
        limit: i64,
    ) -> AppResult<QueryOutcome> {
        let filter_value: JsonValue = serde_json::from_str(filter)
            .map_err(|err| AppError::msg(format!("过滤条件不是 JSON：{err}")))?;
        let filter = bson_from_json(&filter_value)?
            .as_document()
            .cloned()
            .ok_or_else(|| AppError::msg("过滤条件需要是 JSON 对象"))?;
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
            while let Some(doc) = cursor
                .try_next()
                .await
                .map_err(|err| AppError::msg(err.to_string()))?
            {
                docs.push(doc);
            }
            Ok(docs_outcome(docs))
        })
    }

    fn aggregate(
        &self,
        database: &str,
        collection: &str,
        pipeline_json: &str,
    ) -> AppResult<QueryOutcome> {
        let value: JsonValue = serde_json::from_str(pipeline_json)
            .map_err(|err| AppError::msg(format!("管道不是 JSON: {err}")))?;
        let Bson::Array(stages) = bson_from_json(&value)? else {
            return Err(AppError::msg("聚合管道需要是 JSON 数组"));
        };
        let pipeline = stages
            .into_iter()
            .map(|item| match item {
                Bson::Document(doc) => Ok(doc),
                _ => Err(AppError::msg("聚合管道的每一项必须是对象")),
            })
            .collect::<AppResult<Vec<_>>>()?;
        block_on(async {
            let mut cursor = self
                .client
                .database(database)
                .collection::<Document>(collection)
                .aggregate(pipeline, None)
                .await
                .map_err(|err| AppError::msg(err.to_string()))?;
            let mut docs = Vec::new();
            while let Some(doc) = cursor
                .try_next()
                .await
                .map_err(|err| AppError::msg(err.to_string()))?
            {
                docs.push(doc);
            }
            Ok(docs_outcome(docs))
        })
    }
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
    let original_rows = docs
        .iter()
        .map(|doc| {
            doc.iter()
                .map(|(name, value)| FieldValue {
                    column: name.clone(),
                    value: json_from_bson(value),
                })
                .collect()
        })
        .collect();
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
        original_rows: Some(original_rows),
        columns,
        rows,
        rows_affected: 0,
        truncated: false,
    }
}

fn document_outcome(doc: Document) -> QueryOutcome {
    docs_outcome(vec![doc])
}

fn validate_field(name: &str) -> AppResult<()> {
    if name.is_empty() || name.starts_with('$') || name.contains(['.', '\0']) {
        return Err(AppError::msg(
            "字段名包含保留字符，请使用 MongoDB 原生命令处理该字段",
        ));
    }
    Ok(())
}

fn stable_sort(sorts: &[SortSpec]) -> AppResult<Document> {
    if sorts.len() > 3 {
        return Err(AppError::msg("排序最多 3 列"));
    }
    let mut sort = Document::new();
    for item in sorts {
        validate_field(&item.column)?;
        sort.insert(item.column.clone(), if item.desc { -1 } else { 1 });
    }
    if !sort.contains_key("_id") {
        sort.insert("_id", 1);
    }
    Ok(sort)
}

fn filters_to_doc(filters: &[FilterSpec]) -> AppResult<Document> {
    let mut clauses = Vec::new();
    for filter in filters {
        validate_field(&filter.column)?;
        let value = filter
            .value
            .as_ref()
            .map(|text| Bson::String(text.clone()))
            .unwrap_or(Bson::Null);
        let clause = match filter.op.as_str() {
            "eq" => Bson::Document(doc! { "$eq": value }),
            "ne" => Bson::Document(doc! { "$ne": value }),
            "gt" => Bson::Document(doc! { "$gt": value }),
            "lt" => Bson::Document(doc! { "$lt": value }),
            "gte" => Bson::Document(doc! { "$gte": value }),
            "lte" => Bson::Document(doc! { "$lte": value }),
            "contains" => {
                let mut escaped = String::new();
                for ch in filter.value.as_deref().unwrap_or("").chars() {
                    if ".^$*+?()[]{}|\\".contains(ch) {
                        escaped.push('\\');
                    }
                    escaped.push(ch);
                }
                Bson::Document(doc! { "$regex": escaped, "$options": "i" })
            }
            "is_null" => Bson::Null,
            "is_not_null" => Bson::Document(doc! { "$ne": Bson::Null }),
            _ => return Err(AppError::msg("不支持的筛选运算符")),
        };
        let mut item = Document::new();
        item.insert(filter.column.clone(), clause);
        clauses.push(item);
    }
    Ok(if clauses.is_empty() {
        Document::new()
    } else {
        doc! { "$and": clauses }
    })
}

fn conflict() -> AppError {
    AppError::msg("数据已被修改或删除，本条更改未写入；请重新加载后检查更改")
}

fn original_filter(change: &RowChange) -> AppResult<Document> {
    let original = change
        .original
        .as_ref()
        .filter(|fields| !fields.is_empty())
        .ok_or_else(|| AppError::msg("缺少原始文档，请重新加载后修改"))?;
    let id = change
        .pk
        .iter()
        .find(|field| field.column == "_id")
        .ok_or_else(|| AppError::msg("MongoDB 更新和删除需要 _id"))?;
    let original_id = original
        .iter()
        .find(|field| field.column == "_id")
        .ok_or_else(|| AppError::msg("原始文档缺少 _id"))?;
    if original_id.value != id.value {
        return Err(AppError::msg("主键与原始文档不一致"));
    }
    let mut checks = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut present = 0i32;
    for field in original {
        validate_field(&field.column)?;
        if !seen.insert(&field.column) {
            return Err(AppError::msg("原始文档包含重复字段"));
        }
        let reference = format!("${}", field.column);
        present += 1;
        let value = bson_from_json(&field.value)?;
        checks.push(doc! { "$eq": [reference.clone(), { "$literal": value.clone() }] });
        checks.push(doc! { "$eq": [{ "$type": reference }, { "$type": { "$literal": value } }] });
    }
    checks.push(doc! { "$eq": [{ "$size": { "$objectToArray": "$$ROOT" } }, present] });
    Ok(doc! { "_id": { "$eq": bson_from_json(&id.value)? }, "$expr": { "$and": checks } })
}

fn json_from_bson(value: &Bson) -> JsonValue {
    value.clone().into_canonical_extjson()
}

fn bson_from_json(value: &JsonValue) -> AppResult<Bson> {
    Bson::try_from(value.clone()).map_err(|err| AppError::msg(format!("BSON 值无效：{err}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_cells_preserve_bson_types_and_nested_field_order() {
        let document = doc! {
            "long": i64::MAX,
            "small": 7i32,
            "amount": Bson::Decimal128("1234567890.1234567890123456789".parse().unwrap()),
            "date": mongodb::bson::DateTime::from_millis(1234567890),
            "id": mongodb::bson::oid::ObjectId::new(),
            "bytes": Bson::Binary(mongodb::bson::Binary { subtype: mongodb::bson::spec::BinarySubtype::Generic, bytes: vec![0, 255] }),
            "nested": { "z": 1i32, "a": 2i32 },
            "literal": "oid:012345678901234567890123",
        };
        let encoded = json_from_bson(&Bson::Document(document.clone()));
        let decoded = bson_from_json(&encoded)
            .unwrap()
            .as_document()
            .unwrap()
            .clone();
        assert_eq!(
            mongodb::bson::to_vec(&decoded).unwrap(),
            mongodb::bson::to_vec(&document).unwrap()
        );
        assert_eq!(encoded["long"]["$numberLong"], i64::MAX.to_string());
        assert_eq!(
            decoded.get_str("literal").unwrap(),
            "oid:012345678901234567890123"
        );
    }

    #[test]
    fn sparse_original_rows_distinguish_absent_from_explicit_null() {
        let outcome = docs_outcome(vec![
            doc! { "_id": 1, "a": "one" },
            doc! { "_id": 2, "b": Bson::Null },
        ]);
        let originals = outcome.original_rows.unwrap();
        assert_eq!(originals[0].len(), 2);
        assert!(!originals[0].iter().any(|field| field.column == "b"));
        assert!(originals[1]
            .iter()
            .any(|field| field.column == "b" && field.value.is_null()));
        let change = RowChange {
            kind: "delete".into(),
            pk: vec![originals[0][0].clone()],
            original: Some(originals[0].clone()),
            values: vec![],
        };
        let filter = original_filter(&change).unwrap();
        let checks = filter
            .get_document("$expr")
            .unwrap()
            .get_array("$and")
            .unwrap();
        assert_eq!(
            checks
                .last()
                .unwrap()
                .as_document()
                .unwrap()
                .get_array("$eq")
                .unwrap()[1],
            Bson::Int32(2)
        );
        assert!(original_filter(&RowChange {
            original: None,
            ..change
        })
        .is_err());
    }

    #[test]
    fn paging_adds_id_tie_breaker_and_filters_keep_all_clauses() {
        assert_eq!(stable_sort(&[]).unwrap(), doc! { "_id": 1 });
        assert_eq!(
            stable_sort(&[SortSpec {
                column: "label".into(),
                desc: true
            }])
            .unwrap(),
            doc! { "label": -1, "_id": 1 }
        );
        let filters = vec![
            FilterSpec {
                column: "label".into(),
                op: "contains".into(),
                value: Some("a.b".into()),
            },
            FilterSpec {
                column: "label".into(),
                op: "ne".into(),
                value: Some("z".into()),
            },
        ];
        let filter = filters_to_doc(&filters).unwrap();
        let clauses = filter.get_array("$and").unwrap();
        assert_eq!(clauses.len(), 2);
        assert_eq!(
            clauses[0]
                .as_document()
                .unwrap()
                .get_document("label")
                .unwrap()
                .get_str("$regex")
                .unwrap(),
            "a\\.b"
        );
    }
}
