use super::*;
use crate::models::{ExecuteSqlRequest, SaveConnectionRequest};
use crate::query::execute_sql;
use serde_json::json;
use std::fs;
use std::path::PathBuf;

struct Fixture {
    state: AppState,
    directory: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let directory =
            std::env::temp_dir().join(format!("dataharbor-stream-test-{}", uuid::Uuid::new_v4()));
        let state = AppState::open_in(directory.clone(), true).unwrap();
        Self { state, directory }
    }
    fn database(&self, name: &str, sql: &str) -> String {
        let id = self
            .state
            .save_connection(SaveConnectionRequest {
                name: name.into(),
                engine: Engine::Sqlite,
                file_path: self
                    .directory
                    .join(format!("{name}.sqlite"))
                    .display()
                    .to_string(),
                ..Default::default()
            })
            .unwrap()
            .connection
            .id;
        self.state.connect(&id).unwrap();
        self.execute(&id, sql);
        id
    }
    fn execute(&self, id: &str, sql: &str) {
        let result = execute_sql(
            &self.state,
            ExecuteSqlRequest {
                connection_id: id.into(),
                database: "main".into(),
                sql: sql.into(),
                page: 1,
                page_size: 100,
                only_statement: None,
            },
        )
        .unwrap();
        assert!(
            result.statements.iter().all(|statement| statement.ok),
            "{result:?}"
        );
    }
    fn rows(&self, id: &str, sql: &str) -> Vec<Vec<Value>> {
        self.state
            .with_session(id, |session| session.execute("main", sql, None))
            .unwrap()
            .rows
    }
    fn no_stage(&self) {
        let path = self.directory.join("operations");
        assert!(!path.exists() || fs::read_dir(path).unwrap().count() == 0);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for profile in self.state.profiles_snapshot() {
            self.state.disconnect(&profile.id);
        }
        let _ = fs::remove_dir_all(&self.directory);
    }
}
fn pair(source: &str, target: &str) -> PairRequest {
    serde_json::from_value(json!({"sourceId":source,"sourceDatabase":"main","sourceTable":"items","targetId":target,"targetDatabase":"main","targetTable":"items","deleteMissing":true})).unwrap()
}

#[test]
fn full_sync_above_old_ceiling_is_complete_and_only_holds_bounded_batches() {
    let fixture = Fixture::new();
    let source = fixture.database(
        "source",
        "CREATE TABLE items(id INTEGER PRIMARY KEY, value TEXT)",
    );
    let target = fixture.database(
        "target",
        "CREATE TABLE items(value TEXT, id INTEGER PRIMARY KEY)",
    );
    let mut connection =
        rusqlite::Connection::open(fixture.directory.join("source.sqlite")).unwrap();
    {
        let tx = connection.transaction().unwrap();
        {
            let mut insert = tx.prepare("INSERT INTO items VALUES(?1,?2)").unwrap();
            for id in 1..=200_003 {
                insert
                    .execute(rusqlite::params![id, format!("value-{id}")])
                    .unwrap();
            }
        }
        tx.commit().unwrap();
    }
    drop(connection);
    let operation = OperationContext::detached();
    let mut peak = 0;
    let result = run_with_hook(
        &fixture.state,
        &pair(&source, &target),
        true,
        &operation,
        &mut |phase, count| {
            if phase == "batchPeak" {
                peak = count;
            }
        },
    )
    .unwrap();
    assert_eq!(result.inserted, 200_003);
    assert!(!result.truncated);
    assert_eq!(operation.snapshot().committed, 200_003);
    assert_eq!(
        fixture.rows(&target, "SELECT COUNT(*) FROM items"),
        vec![vec![json!(200_003)]]
    );
    assert_eq!(
        fixture.rows(&target, "SELECT value FROM items WHERE id=200003"),
        vec![vec![json!("value-200003")]]
    );
    assert!(
        peak > 0 && peak <= u64::from(BATCH),
        "maximum retained batch: {peak}"
    );
    fixture.no_stage();
}

#[test]
fn reading_and_comparing_cancellation_never_start_target_writes() {
    let fixture = Fixture::new();
    let source=fixture.database("cancel-source","CREATE TABLE items(id INTEGER PRIMARY KEY,value TEXT); WITH RECURSIVE x(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM x WHERE n<1200) INSERT INTO items SELECT n,'source' FROM x");
    let target=fixture.database("cancel-target","CREATE TABLE items(id INTEGER PRIMARY KEY,value TEXT); INSERT INTO items VALUES(9000,'keep')");
    for cancel_phase in ["读取源表", "读取目标表", "对比差异", "校验变更"] {
        let operation = OperationContext::detached();
        let result = run_with_hook(
            &fixture.state,
            &pair(&source, &target),
            true,
            &operation,
            &mut |phase, _| {
                if phase == cancel_phase {
                    operation.request_cancel().unwrap();
                }
            },
        );
        assert!(
            matches!(result, Err(AppError::Cancelled { committed: 0, .. })),
            "phase {cancel_phase}: {result:?}"
        );
        assert_eq!(
            fixture.rows(&target, "SELECT * FROM items"),
            vec![vec![json!(9000), json!("keep")]]
        );
        fixture.no_stage();
    }
}

#[test]
fn cancellation_after_a_committed_batch_stops_later_writes_and_keeps_accurate_counts() {
    let fixture = Fixture::new();
    let source=fixture.database("write-source","CREATE TABLE items(id INTEGER PRIMARY KEY,value TEXT); WITH RECURSIVE x(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM x WHERE n<1200) INSERT INTO items SELECT n,'source' FROM x");
    let target = fixture.database(
        "write-target",
        "CREATE TABLE items(id INTEGER PRIMARY KEY,value TEXT)",
    );
    let operation = OperationContext::detached();
    let result = run_with_hook(
        &fixture.state,
        &pair(&source, &target),
        true,
        &operation,
        &mut |phase, count| {
            if phase == "写入目标" && count >= u64::from(BATCH) {
                operation.request_cancel().unwrap();
            }
        },
    );
    assert!(
        matches!(
            result,
            Err(AppError::Cancelled {
                committed: 500,
                batches: 1
            })
        ),
        "{result:?}"
    );
    assert_eq!(operation.snapshot().committed, 500);
    assert_eq!(
        fixture.rows(&target, "SELECT COUNT(*) FROM items"),
        vec![vec![json!(500)]]
    );
    fixture.no_stage();
}

#[test]
fn late_invalid_value_is_rejected_in_preflight_before_any_target_write() {
    let fixture = Fixture::new();
    let source=fixture.database("invalid-source","CREATE TABLE items(id INTEGER PRIMARY KEY,value TEXT); WITH RECURSIVE x(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM x WHERE n<600) INSERT INTO items SELECT n,'123' FROM x; UPDATE items SET value='not-a-number' WHERE id=600");
    let target=fixture.database("invalid-target","CREATE TABLE items(id INTEGER PRIMARY KEY,value DECIMAL(10,2)); INSERT INTO items VALUES(9000,9)");
    assert!(run(
        &fixture.state,
        &pair(&source, &target),
        true,
        &OperationContext::detached()
    )
    .is_err());
    assert_eq!(
        fixture.rows(&target, "SELECT * FROM items"),
        vec![vec![json!(9000), json!(9)]]
    );
    fixture.no_stage();
}

#[test]
fn streamed_composite_keys_do_not_depend_on_cross_table_order_or_separators() {
    let fixture = Fixture::new();
    let source=fixture.database("composite-source","CREATE TABLE items(a TEXT,b TEXT,value TEXT,PRIMARY KEY(a,b)); INSERT INTO items VALUES('a|b','c','first'),('a','b|c','second'),('z','1','last')");
    let target=fixture.database("composite-target","CREATE TABLE items(value TEXT,b TEXT,a TEXT,PRIMARY KEY(b,a)); INSERT INTO items VALUES('old','c','a|b'),('gone','x','x')");
    let result = run(
        &fixture.state,
        &pair(&source, &target),
        true,
        &OperationContext::detached(),
    )
    .unwrap();
    assert_eq!((result.inserted, result.updated, result.deleted), (2, 1, 1));
    assert_eq!(
        fixture.rows(&target, "SELECT a,b,value FROM items ORDER BY a,b"),
        fixture.rows(&source, "SELECT a,b,value FROM items ORDER BY a,b")
    );
    fixture.no_stage();
}

#[test]
fn source_snapshot_does_not_change_when_source_is_written_after_first_page() {
    let fixture = Fixture::new();
    let source=fixture.database("snapshot-source","PRAGMA journal_mode=WAL; CREATE TABLE items(id INTEGER PRIMARY KEY,value TEXT); WITH RECURSIVE x(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM x WHERE n<700) INSERT INTO items SELECT n,'before' FROM x");
    let target = fixture.database(
        "snapshot-target",
        "CREATE TABLE items(id INTEGER PRIMARY KEY,value TEXT)",
    );
    let mut changed = false;
    let result = run_with_hook(
        &fixture.state,
        &pair(&source, &target),
        true,
        &OperationContext::detached(),
        &mut |phase, count| {
            if phase == "读取源表" && count == 500 && !changed {
                let other =
                    rusqlite::Connection::open(fixture.directory.join("snapshot-source.sqlite"))
                        .unwrap();
                other
                    .execute("UPDATE items SET value='after' WHERE id=700", [])
                    .unwrap();
                other
                    .execute("INSERT INTO items VALUES(701,'new')", [])
                    .unwrap();
                changed = true;
            }
        },
    )
    .unwrap();
    assert_eq!(result.inserted, 700);
    assert_eq!(
        fixture.rows(&target, "SELECT value FROM items WHERE id=700"),
        vec![vec![json!("before")]]
    );
    assert_eq!(
        fixture.rows(&target, "SELECT COUNT(*) FROM items WHERE id=701"),
        vec![vec![json!(0)]]
    );
    fixture.no_stage();
}

#[test]
fn stream_uses_its_own_transaction_and_keeps_the_selected_target_identity() {
    let fixture = Fixture::new();
    let source=fixture.database("pinned-source","CREATE TABLE items(id INTEGER PRIMARY KEY,value TEXT); INSERT INTO items VALUES(1,'source')");
    let target = fixture.database(
        "pinned-target",
        "CREATE TABLE items(id INTEGER PRIMARY KEY,value TEXT)",
    );
    let other=fixture.database("other-target","CREATE TABLE items(id INTEGER PRIMARY KEY,value TEXT); INSERT INTO items VALUES(99,'untouched')");
    fixture.execute(&target, "BEGIN");
    run(
        &fixture.state,
        &pair(&source, &target),
        true,
        &OperationContext::detached(),
    )
    .unwrap();
    fixture.execute(&target, "ROLLBACK");
    fixture.execute(&target, "DELETE FROM items");
    let mut changed = false;
    run_with_hook(
        &fixture.state,
        &pair(&source, &target),
        true,
        &OperationContext::detached(),
        &mut |phase, _| {
            if phase == "校验变更" && !changed {
                fixture
                    .state
                    .save_connection(SaveConnectionRequest {
                        id: Some(target.clone()),
                        name: "changed target".into(),
                        engine: Engine::Sqlite,
                        file_path: fixture
                            .directory
                            .join("other-target.sqlite")
                            .display()
                            .to_string(),
                        ..Default::default()
                    })
                    .unwrap();
                fixture.state.connect(&target).unwrap();
                changed = true;
            }
        },
    )
    .unwrap();
    assert!(changed);
    assert_eq!(
        fixture.rows(&other, "SELECT * FROM items"),
        vec![vec![json!(99), json!("untouched")]]
    );
    let old_target =
        rusqlite::Connection::open(fixture.directory.join("pinned-target.sqlite")).unwrap();
    let value: String = old_target
        .query_row("SELECT value FROM items WHERE id=1", [], |row| row.get(0))
        .unwrap();
    assert_eq!(value, "source");
    fixture.no_stage();
}

#[test]
fn nocase_primary_keys_are_refused_before_any_target_write() {
    let fixture = Fixture::new();
    let binary=fixture.database("binary-keys","CREATE TABLE items(id TEXT PRIMARY KEY,value TEXT); INSERT INTO items VALUES('A','one'),('a','two')");
    let nocase=fixture.database("nocase-keys","CREATE TABLE items(id TEXT COLLATE NOCASE PRIMARY KEY,value TEXT); INSERT INTO items VALUES('keep','unchanged')");
    for request in [pair(&binary, &nocase), pair(&nocase, &binary)] {
        let before_binary = fixture.rows(&binary, "SELECT * FROM items ORDER BY id");
        let before_nocase = fixture.rows(&nocase, "SELECT * FROM items ORDER BY id");
        let error = run(
            &fixture.state,
            &request,
            true,
            &OperationContext::detached(),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("排序规则"), "{error}");
        assert_eq!(
            fixture.rows(&binary, "SELECT * FROM items ORDER BY id"),
            before_binary
        );
        assert_eq!(
            fixture.rows(&nocase, "SELECT * FROM items ORDER BY id"),
            before_nocase
        );
        fixture.no_stage();
    }
}

#[test]
fn explicit_binary_primary_index_controls_keyset_comparison_not_column_nocase() {
    let fixture = Fixture::new();
    let source = fixture.database(
        "binary-index-source",
        "CREATE TABLE items(id TEXT COLLATE NOCASE,value TEXT,PRIMARY KEY(id COLLATE BINARY))",
    );
    let target = fixture.database(
        "binary-index-target",
        "CREATE TABLE items(id TEXT PRIMARY KEY,value TEXT)",
    );
    let mut rows = (1..=501)
        .map(|n| format!("('k{n:04}','value')"))
        .collect::<Vec<_>>();
    rows.push("('K0500','different case')".into());
    fixture.execute(
        &source,
        &format!("INSERT INTO items VALUES {}", rows.join(",")),
    );
    let result = run(
        &fixture.state,
        &pair(&source, &target),
        true,
        &OperationContext::detached(),
    )
    .unwrap();
    assert_eq!(result.inserted, 502);
    assert_eq!(
        fixture.rows(&target, "SELECT COUNT(*) FROM items"),
        vec![vec![json!(502)]]
    );
    assert_eq!(
        fixture.rows(&target, "SELECT value FROM items WHERE id='K0500'"),
        vec![vec![json!("different case")]]
    );
    fixture.no_stage();
}

struct ServerFixture {
    state: AppState,
    id: String,
    database: String,
    schema: String,
    source: String,
    target: String,
    source_sql: String,
    target_sql: String,
    domain: String,
    directory: PathBuf,
}
impl ServerFixture {
    fn new(engine: Engine, variable: &str) -> Self {
        let port = std::env::var(variable)
            .expect("需要专用测试数据库端口")
            .parse()
            .unwrap();
        let directory =
            std::env::temp_dir().join(format!("dataharbor-stream-server-{}", uuid::Uuid::new_v4()));
        let state = AppState::open_in(directory.clone(), true).unwrap();
        let id = state
            .save_connection(SaveConnectionRequest {
                name: "流式测试".into(),
                engine,
                host: "127.0.0.1".into(),
                port,
                username: if engine == Engine::Postgres {
                    "postgres"
                } else {
                    "root"
                }
                .into(),
                password: Some("dataharbor".into()),
                database: "demo".into(),
                ..Default::default()
            })
            .unwrap()
            .connection
            .id;
        state.connect(&id).unwrap();
        let prefix = format!("dh_stream_{}", uuid::Uuid::new_v4().simple());
        let schema = if engine == Engine::Postgres {
            "public"
        } else {
            ""
        }
        .to_string();
        let source = format!("{prefix}_source");
        let target = format!("{prefix}_target");
        let source_sql = crate::sqlutil::qualified_name(engine, "demo", &schema, &source).unwrap();
        let target_sql = crate::sqlutil::qualified_name(engine, "demo", &schema, &target).unwrap();
        Self {
            state,
            id,
            database: "demo".into(),
            schema,
            source,
            target,
            source_sql,
            target_sql,
            domain: format!("{prefix}_key"),
            directory,
        }
    }
    fn sql(&self, sql: &str) -> Vec<Vec<Value>> {
        self.state
            .with_session(&self.id, |session| {
                session.execute(&self.database, sql, None)
            })
            .unwrap()
            .rows
    }
    fn request(&self) -> PairRequest {
        serde_json::from_value(json!({"sourceId":self.id,"sourceDatabase":self.database,"sourceSchema":self.schema,"sourceTable":self.source,"targetId":self.id,"targetDatabase":self.database,"targetSchema":self.schema,"targetTable":self.target,"deleteMissing":true})).unwrap()
    }
}
impl Drop for ServerFixture {
    fn drop(&mut self) {
        for table in [&self.source_sql, &self.target_sql] {
            let _ = self.state.with_session(&self.id, |session| {
                session.execute(
                    &self.database,
                    &format!("DROP TABLE IF EXISTS {table}"),
                    None,
                )
            });
        }
        if !self.schema.is_empty() {
            let _ = self.state.with_session(&self.id, |session| {
                session.execute(
                    &self.database,
                    &format!("DROP DOMAIN IF EXISTS {}", self.domain),
                    None,
                )
            });
        }
        self.state.disconnect(&self.id);
        let _ = fs::remove_dir_all(&self.directory);
    }
}
fn server_stream_roundtrip(engine: Engine, variable: &str) {
    let fixture = ServerFixture::new(engine, variable);
    let key = if engine == Engine::Mysql {
        "VARCHAR(30) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_bin"
    } else {
        "VARCHAR(30)"
    };
    fixture.sql(&format!(
        "CREATE TABLE {}(tenant {key}, id DECIMAL(20,0), value TEXT, PRIMARY KEY(tenant,id))",
        fixture.source_sql
    ));
    fixture.sql(&format!(
        "CREATE TABLE {}(value TEXT, id DECIMAL(20,0), tenant {key}, PRIMARY KEY(id,tenant))",
        fixture.target_sql
    ));
    for chunk in (0..1003).collect::<Vec<_>>().chunks(200) {
        let rows = chunk
            .iter()
            .map(|n| {
                format!(
                    "('{}',{},'value-{n}')",
                    if n % 2 == 0 { "A" } else { "a" },
                    9_007_199_254_740_993i64 + i64::from(*n)
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        fixture.sql(&format!("INSERT INTO {} VALUES {rows}", fixture.source_sql));
    }
    fixture.sql(&format!(
        "INSERT INTO {} VALUES('old',9007199254740993,'A'),('extra',1,'z')",
        fixture.target_sql
    ));
    let operation = OperationContext::detached();
    let result = run(&fixture.state, &fixture.request(), true, &operation).unwrap();
    assert_eq!(
        (result.inserted, result.updated, result.deleted),
        (1002, 1, 1)
    );
    let count = fixture.sql(&format!("SELECT COUNT(*) FROM {}", fixture.target_sql));
    assert_eq!(crate::sqlutil::json_count(&count[0][0]), Some(1003));
    assert_eq!(
        fixture.sql(&format!(
            "SELECT value FROM {} WHERE id=9007199254740993 AND tenant='A'",
            fixture.target_sql
        )),
        vec![vec![json!("value-0")]]
    );
    assert_eq!(operation.snapshot().committed, 1004);
    if engine == Engine::Mysql {
        fixture.sql(&format!("ALTER TABLE {} MODIFY tenant VARCHAR(30) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL",fixture.source_sql));
    } else {
        fixture.sql(&format!("CREATE DOMAIN {} AS VARCHAR(30)", fixture.domain));
        fixture.sql(&format!(
            "ALTER TABLE {} ALTER COLUMN tenant TYPE {}",
            fixture.source_sql, fixture.domain
        ));
    }
    let error = run(
        &fixture.state,
        &fixture.request(),
        true,
        &OperationContext::detached(),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("排序规则"), "{error}");
    assert_eq!(
        fixture.sql(&format!("SELECT COUNT(*) FROM {}", fixture.target_sql)),
        count
    );
    assert_eq!(
        fs::read_dir(fixture.directory.join("operations"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
#[ignore = "需要专用MySQL测试容器"]
fn mysql_streaming_snapshots_and_no_pad_keys_round_trip() {
    server_stream_roundtrip(Engine::Mysql, "DATAHARBOR_TEST_MYSQL_PORT");
}

#[test]
#[ignore = "需要专用PostgreSQL测试容器"]
fn postgres_streaming_snapshots_and_builtin_keys_round_trip() {
    server_stream_roundtrip(Engine::Postgres, "DATAHARBOR_TEST_POSTGRES_PORT");
}
