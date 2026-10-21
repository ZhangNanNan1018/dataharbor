//! Opt-in tests for disposable localhost MongoDB/Redis containers only.
use crate::data::{commit_changes, fetch_rows};
use crate::models::*;
use crate::state::AppState;
use serde_json::{json, Value};
use std::{fs, path::PathBuf};
use uuid::Uuid;

struct Fixture {
    state: AppState,
    id: String,
    database: String,
    prefix: String,
    engine: Engine,
    dir: PathBuf,
}

impl Fixture {
    fn open(engine: Engine, variable: &str) -> Self {
        let port: u16 = std::env::var(variable)
            .expect("请设置专用测试容器端口")
            .parse()
            .unwrap();
        assert!(![27017, 6379].contains(&port), "不要指向默认业务数据库端口");
        let prefix = format!("dh_test_{}", Uuid::new_v4().simple());
        let database = if engine == Engine::Mongodb {
            prefix.clone()
        } else {
            "0".into()
        };
        let dir = std::env::temp_dir().join(&prefix);
        let state = AppState::open_in(dir.clone(), true).unwrap();
        let id = state
            .save_connection(SaveConnectionRequest {
                name: "文档库临时测试".into(),
                engine,
                host: "127.0.0.1".into(),
                port,
                database: database.clone(),
                ..Default::default()
            })
            .unwrap()
            .connection
            .id;
        state.connect(&id).unwrap();
        Self {
            state,
            id,
            database,
            prefix,
            engine,
            dir,
        }
    }

    fn command(&self, text: &str) -> QueryOutcome {
        self.state
            .with_session(&self.id, |session| {
                session.execute(&self.database, text, Some(1000))
            })
            .unwrap()
    }

    fn page(&self, table: &str, filters: Vec<FilterSpec>, page: u32, page_size: u32) -> DataPage {
        fetch_rows(
            &self.state,
            FetchRowsRequest {
                connection_id: self.id.clone(),
                database: self.database.clone(),
                schema: String::new(),
                table: table.into(),
                filters,
                sorts: vec![],
                page,
                page_size,
            },
        )
        .unwrap()
    }

    fn change(
        &self,
        table: &str,
        page: &DataPage,
        index: usize,
        kind: &str,
        values: Vec<FieldValue>,
    ) -> crate::error::AppResult<CommitResult> {
        let original = page
            .original_rows
            .as_ref()
            .map(|rows| rows[index].clone())
            .unwrap_or_else(|| {
                page.columns
                    .iter()
                    .zip(&page.rows[index])
                    .map(|(column, value)| field(&column.name, value.clone()))
                    .collect()
            });
        let pk = page
            .columns
            .iter()
            .zip(&page.rows[index])
            .filter(|(column, _)| column.primary_key)
            .map(|(column, value)| field(&column.name, value.clone()))
            .collect();
        commit_changes(
            &self.state,
            CommitRequest {
                connection_id: self.id.clone(),
                database: self.database.clone(),
                schema: String::new(),
                table: table.into(),
                changes: vec![RowChange {
                    kind: kind.into(),
                    pk,
                    original: Some(original),
                    values,
                }],
            },
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.engine == Engine::Mongodb {
            let _ = self.state.with_session(&self.id, |session| {
                session.execute(&self.database, "{\"dropDatabase\":1}", None)
            });
        } else {
            for suffix in ["string", "hash", "list", "set", "zset", "typed", "long"] {
                let _ = self.state.with_session(&self.id, |session| {
                    session.execute("0", &format!("DEL {}_{suffix}", self.prefix), None)
                });
            }
        }
        self.state.disconnect(&self.id);
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn field(name: &str, value: Value) -> FieldValue {
    FieldValue {
        column: name.into(),
        value,
    }
}

#[test]
#[ignore = "requires disposable MongoDB at DATAHARBOR_TEST_MONGO_PORT"]
fn mongo_filters_sparse_snapshots_and_conflicts_round_trip() {
    let fixture = Fixture::open(Engine::Mongodb, "DATAHARBOR_TEST_MONGO_PORT");
    let table = "documents";
    fixture.command(&json!({"insert": table, "documents": [
        {"_id":{"$numberLong":"1"},"category":"keep","title":"first","large":{"$numberLong":"9223372036854775807"},"date":{"$date":{"$numberLong":"123456789"}},"nested":{"z":1,"a":2}},
        {"_id":{"$numberLong":"2"},"category":"keep","title":"second","optional":null},
        {"_id":{"$numberLong":"3"},"category":"skip","title":"third"}
    ]}).to_string());
    let filter = || {
        vec![FilterSpec {
            column: "category".into(),
            op: "eq".into(),
            value: Some("keep".into()),
        }]
    };
    let first = fixture.page(table, filter(), 1, 1);
    let second = fixture.page(table, filter(), 2, 1);
    assert_eq!(first.total, 2);
    assert_eq!(second.total, 2);
    assert_eq!(
        first.original_rows.as_ref().unwrap()[0][0].value,
        json!({"$numberLong":"1"})
    );
    assert_eq!(
        second.original_rows.as_ref().unwrap()[0][0].value,
        json!({"$numberLong":"2"})
    );
    assert!(!first.original_rows.as_ref().unwrap()[0]
        .iter()
        .any(|field| field.column == "optional"));
    fixture
        .change(
            table,
            &first,
            0,
            "update",
            vec![field("title", json!("edited"))],
        )
        .unwrap();
    let reread = fixture.page(table, filter(), 1, 1);
    let original = &reread.original_rows.as_ref().unwrap()[0];
    assert_eq!(
        original
            .iter()
            .find(|field| field.column == "large")
            .unwrap()
            .value,
        json!({"$numberLong":"9223372036854775807"})
    );
    assert_eq!(
        original
            .iter()
            .find(|field| field.column == "date")
            .unwrap()
            .value,
        json!({"$date":{"$numberLong":"123456789"}})
    );
    assert!(fixture
        .change(
            table,
            &first,
            0,
            "update",
            vec![field("title", json!("stale"))]
        )
        .is_err());
    assert!(fixture.change(table, &first, 0, "delete", vec![]).is_err());
    // Adding a formerly absent field is also a conflict, even when its value is null.
    fixture.command(&json!({"update":table,"updates":[{"q":{"_id":{"$numberLong":"1"}},"u":{"$set":{"added":null}}}]}).to_string());
    assert!(fixture.change(table, &reread, 0, "delete", vec![]).is_err());
    let latest = fixture.page(table, filter(), 1, 1);
    fixture.change(table, &latest, 0, "delete", vec![]).unwrap();
    assert!(fixture.change(table, &latest, 0, "delete", vec![]).is_err());
    assert_eq!(fixture.page(table, filter(), 1, 10).total, 1);
    fixture.command(&json!({"createIndexes": table, "indexes": [{"key": {"title": 1}, "name": "title_unique", "unique": true}]}).to_string());
    let before = fixture.page(table, vec![], 1, 10).total;
    let insert = || RowChange { kind: "insert".into(), original: None, pk: vec![], values: vec![field("title", json!("duplicate fixture"))] };
    let error = commit_changes(&fixture.state, CommitRequest {
        connection_id: fixture.id.clone(), database: fixture.database.clone(), schema: String::new(),
        table: table.into(), changes: vec![insert(), insert()],
    }).unwrap_err();
    assert!(matches!(error, crate::error::AppError::PartialWrite { applied: 1, failed_index: 2, .. }));
    let wire = serde_json::to_value(&error).unwrap();
    assert_eq!(wire["kind"], "partialWrite");
    assert_eq!(wire["applied"], 1);
    assert_eq!(fixture.page(table, vec![], 1, 10).total, before + 1);
}

#[test]
#[ignore = "requires disposable Redis 6+ at DATAHARBOR_TEST_REDIS_PORT"]
fn redis_single_key_cas_preserves_new_values_type_ttl_and_list_members() {
    let fixture = Fixture::open(Engine::Redis, "DATAHARBOR_TEST_REDIS_PORT");
    let key = |suffix: &str| format!("{}_{suffix}", fixture.prefix);
    let string = key("string");
    fixture.command(&format!("SET {string} old PX 30000"));
    let original = fixture.page(&string, vec![], 1, 10);
    fixture
        .change(
            &string,
            &original,
            0,
            "update",
            vec![field("value", json!("new"))],
        )
        .unwrap();
    assert!(
        fixture.command(&format!("PTTL {string}")).rows[0][1]
            .as_i64()
            .unwrap()
            > 0
    );
    assert!(fixture
        .change(
            &string,
            &original,
            0,
            "update",
            vec![field("value", json!("stale"))]
        )
        .is_err());
    assert!(fixture
        .change(&string, &original, 0, "delete", vec![])
        .is_err());
    assert_eq!(
        fixture.page(&string, vec![], 1, 10).rows[0][1],
        json!("new")
    );

    let hash = key("hash");
    fixture.command(&format!("HSET {hash} field old"));
    let original = fixture.page(&hash, vec![], 1, 10);
    fixture.command(&format!("HSET {hash} field external"));
    assert!(fixture
        .change(
            &hash,
            &original,
            0,
            "update",
            vec![field("value", json!("stale"))]
        )
        .is_err());
    let fresh = fixture.page(&hash, vec![], 1, 10);
    fixture
        .change(
            &hash,
            &fresh,
            0,
            "update",
            vec![field("value", json!("fresh"))],
        )
        .unwrap();

    let list = key("list");
    fixture.command(&format!("RPUSH {list} same same tail"));
    let original = fixture.page(&list, vec![], 1, 10);
    fixture
        .change(&list, &original, 1, "delete", vec![])
        .unwrap();
    let remaining = fixture.page(&list, vec![], 1, 10);
    assert_eq!(remaining.total, 2);
    assert_eq!(remaining.rows[0][1], json!("same"));
    assert_eq!(remaining.rows[1][1], json!("tail"));
    assert!(fixture
        .change(
            &list,
            &original,
            1,
            "update",
            vec![field("value", json!("stale"))]
        )
        .is_err());

    let zset = key("zset");
    fixture.command(&format!("ZADD {zset} 1 member"));
    let original = fixture.page(&zset, vec![], 1, 10);
    fixture.command(&format!("ZADD {zset} 2 member"));
    assert!(fixture
        .change(&zset, &original, 0, "delete", vec![])
        .is_err());
    let fresh = fixture.page(&zset, vec![], 1, 10);
    fixture
        .change(&zset, &fresh, 0, "update", vec![field("score", json!(3))])
        .unwrap();
    assert_eq!(fixture.page(&zset, vec![], 1, 10).rows[0][1], json!("3"));

    let typed = key("typed");
    fixture.command(&format!("SET {typed} old"));
    let original = fixture.page(&typed, vec![], 1, 10);
    fixture.command(&format!("DEL {typed}"));
    fixture.command(&format!("HSET {typed} field external"));
    assert!(fixture
        .change(&typed, &original, 0, "delete", vec![])
        .is_err());
    assert_eq!(
        fixture.page(&typed, vec![], 1, 10).rows[0][1],
        json!("external")
    );

    let long = key("long");
    fixture.command(&format!(
        "RPUSH {long} {}",
        (0..600)
            .map(|index| index.to_string())
            .collect::<Vec<_>>()
            .join(" ")
    ));
    let later = fixture.page(&long, vec![], 56, 10);
    assert_eq!(later.total, 600);
    assert_eq!(later.rows[0], vec![json!(550), json!("550")]);
}
