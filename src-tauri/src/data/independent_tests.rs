//! Exercise actual SQLite connections: automatic writes must own their session.
use super::*;
use crate::models::{ExecuteSqlRequest, SaveConnectionRequest};
use serde_json::json;
use std::path::PathBuf;

struct Fixture {
    state: AppState,
    id: String,
    directory: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("dataharbor-isolation-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let state = AppState::open_in(directory.clone(), true).unwrap();
        let id = state.save_connection(SaveConnectionRequest {
            name: "独立会话回归".into(), engine: Engine::Sqlite,
            file_path: directory.join("isolation.sqlite").display().to_string(),
            ..Default::default()
        }).unwrap().connection.id;
        state.connect(&id).unwrap();
        let fixture = Self { state, id, directory };
        for sql in [
            "PRAGMA journal_mode = WAL",
            "CREATE TABLE marker (value INTEGER NOT NULL)",
            "INSERT INTO marker VALUES (0)",
            "CREATE TABLE items (id INTEGER PRIMARY KEY, value TEXT NOT NULL)",
        ] { fixture.editor(sql).unwrap(); }
        fixture
    }

    fn editor(&self, sql: &str) -> AppResult<crate::models::QueryOutcome> {
        self.state.with_session(&self.id, |session| session.execute("main", sql, None))
    }

    fn external(&self, sql: &str) -> Vec<Vec<JsonValue>> {
        self.state.open_isolated_session(&self.id).unwrap().execute("main", sql, None).unwrap().rows
    }

    fn grid(&self, id: i64) -> AppResult<CommitResult> {
        commit_changes(&self.state, CommitRequest {
            connection_id: self.id.clone(), database: "main".into(), schema: "main".into(), table: "items".into(),
            changes: vec![RowChange {
                kind: "insert".into(), original: None, pk: vec![],
                values: vec![FieldValue { column: "id".into(), value: json!(id) }, FieldValue { column: "value".into(), value: json!("grid") }],
            }],
        })
    }

    fn csv(&self, text: &str) -> AppResult<ImportResult> {
        let path = self.directory.join("input.csv");
        fs::write(&path, text).unwrap();
        import_table_csv(&self.state, ImportCsvRequest {
            connection_id: self.id.clone(), database: "main".into(), schema: "main".into(), table: "items".into(),
            path: path.display().to_string(), truncate: false,
        })
    }

    fn assert_editor_transaction_open(&self) {
        let error = self.editor("BEGIN").unwrap_err().to_string();
        assert!(error.contains("transaction"), "{error}");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.state.disconnect(&self.id);
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn grid_and_csv_commit_their_own_writes_without_finishing_editor_read_transaction() {
    let fixture = Fixture::new();
    fixture.editor("BEGIN").unwrap();
    fixture.editor("SELECT value FROM marker").unwrap(); // Establish a WAL snapshot.
    assert_eq!(fixture.grid(1).unwrap().applied, 1);
    assert_eq!(fixture.csv("id,value\n2,csv\n").unwrap().inserted, 1);
    assert_eq!(fixture.external("SELECT id FROM items ORDER BY id"), vec![vec![json!(1)], vec![json!(2)]]);
    fixture.assert_editor_transaction_open();
    fixture.editor("ROLLBACK").unwrap();
    // Rolling back the editor must not undo already committed grid/import work.
    assert_eq!(fixture.editor("SELECT COUNT(*) FROM items").unwrap().rows, vec![vec![json!(2)]]);
}

#[test]
fn failed_grid_and_csv_roll_back_only_their_own_batch() {
    let fixture = Fixture::new();
    fixture.grid(1).unwrap();
    fixture.editor("BEGIN").unwrap();
    fixture.editor("SELECT value FROM marker").unwrap();
    assert!(fixture.grid(1).is_err());
    // First CSV row succeeds, second conflicts; both must roll back together.
    assert!(fixture.csv("id,value\n2,temporary\n1,conflict\n").is_err());
    assert_eq!(fixture.external("SELECT id FROM items"), vec![vec![json!(1)]]);
    fixture.assert_editor_transaction_open();
    fixture.editor("ROLLBACK").unwrap();
}

#[test]
fn editor_uncommitted_write_is_neither_committed_nor_rolled_back_on_isolated_write_lock_failure() {
    let fixture = Fixture::new();
    fixture.editor("BEGIN").unwrap();
    fixture.editor("UPDATE marker SET value = 55").unwrap();
    for error in [fixture.grid(1).unwrap_err().to_string(), fixture.csv("id,value\n2,csv\n").unwrap_err().to_string()] {
        assert!(error.contains("锁定") && error.contains("原查询事务保持不变"), "{error}");
    }
    assert_eq!(fixture.editor("SELECT value FROM marker").unwrap().rows, vec![vec![json!(55)]]);
    assert_eq!(fixture.external("SELECT value FROM marker"), vec![vec![json!(0)]]);
    assert!(fixture.external("SELECT id FROM items").is_empty());
    fixture.assert_editor_transaction_open();
    fixture.editor("ROLLBACK").unwrap();
    assert_eq!(fixture.external("SELECT value FROM marker"), vec![vec![json!(0)]]);
}

#[test]
fn bi_uses_committed_snapshot_and_leaves_editor_writes_and_transaction_untouched() {
    let fixture = Fixture::new();
    fixture.editor("BEGIN").unwrap();
    fixture.editor("UPDATE marker SET value = 55").unwrap();
    let query = |sql: &str| crate::studio::execute_read_query_with_operation(
        &fixture.state,
        ExecuteSqlRequest { connection_id: fixture.id.clone(), database: "main".into(), sql: sql.into(), page: 1, page_size: 10, only_statement: None },
        &OperationContext::detached(),
    );
    let response = query("WITH x AS (SELECT value FROM marker) SELECT value FROM x").unwrap();
    assert_eq!(response.statements[0].rows, vec![vec![json!(0)]]);
    for sql in ["UPDATE marker SET value=1", "SELECT 1; DELETE FROM marker", "WITH x AS (DELETE FROM marker RETURNING *) SELECT * FROM x"] {
        assert!(query(sql).is_err(), "{sql}");
    }
    assert_eq!(fixture.editor("SELECT value FROM marker").unwrap().rows, vec![vec![json!(55)]]);
    assert_eq!(fixture.external("SELECT value FROM marker"), vec![vec![json!(0)]]);
    fixture.assert_editor_transaction_open();
    fixture.editor("ROLLBACK").unwrap();
}

struct ServerFixture {
    state: AppState,
    id: String,
    directory: PathBuf,
    schema: String,
    marker: String,
    items: String,
}

impl ServerFixture {
    fn numbers(rows: Vec<Vec<JsonValue>>) -> Vec<Vec<JsonValue>> {
        rows.into_iter().map(|row| row.into_iter().map(|value| {
            // Server drivers intentionally preserve exact numeric text.
            json!(crate::sqlutil::json_count(&value).expect("integer fixture cell"))
        }).collect()).collect()
    }

    fn new(engine: Engine) -> Self {
        let unique = uuid::Uuid::new_v4().simple().to_string();
        let directory = std::env::temp_dir().join(format!("dataharbor-server-isolation-{unique}"));
        fs::create_dir_all(&directory).unwrap();
        let state = AppState::open_in(directory.clone(), true).unwrap();
        let id = state.save_connection(SaveConnectionRequest {
            name: "独立事务实库回归".into(), engine, host: "127.0.0.1".into(),
            // Opt-in tests never fall back to a possibly unrelated local DB.
            port: std::env::var(if engine == Engine::Postgres { "DATAHARBOR_TEST_POSTGRES_PORT" } else { "DATAHARBOR_TEST_MYSQL_PORT" })
                .expect("set the isolated database test port explicitly").parse().expect("valid test port"),
            username: if engine == Engine::Postgres { "postgres" } else { "root" }.into(),
            password: Some("dataharbor".into()), database: "demo".into(),
            ..Default::default()
        }).unwrap().connection.id;
        state.connect(&id).unwrap();
        let fixture = Self { state, id, directory, schema: if engine == Engine::Postgres { "public" } else { "" }.into(), marker: format!("dh_marker_{unique}"), items: format!("dh_items_{unique}") };
        fixture.editor(&format!("CREATE TABLE {} (id INTEGER PRIMARY KEY, value INTEGER NOT NULL)", fixture.marker)).unwrap();
        fixture.editor(&format!("INSERT INTO {} VALUES (1,0)", fixture.marker)).unwrap();
        fixture.editor(&format!("CREATE TABLE {} (id INTEGER PRIMARY KEY, value TEXT NOT NULL)", fixture.items)).unwrap();
        fixture
    }

    fn editor(&self, sql: &str) -> AppResult<crate::models::QueryOutcome> {
        self.state.with_session(&self.id, |session| session.execute("demo", sql, None))
    }

    fn external(&self, sql: &str) -> Vec<Vec<JsonValue>> {
        Self::numbers(self.state.open_isolated_session(&self.id).unwrap().execute("demo", sql, None).unwrap().rows)
    }

    fn grid(&self, id: i64) -> AppResult<CommitResult> {
        commit_changes(&self.state, CommitRequest {
            connection_id: self.id.clone(), database: "demo".into(), schema: self.schema.clone(), table: self.items.clone(),
            changes: vec![RowChange { kind: "insert".into(), original: None, pk: vec![], values: vec![FieldValue { column: "id".into(), value: json!(id) }, FieldValue { column: "value".into(), value: json!("grid") }] }],
        })
    }

    fn csv(&self, text: &str) -> AppResult<ImportResult> {
        let path = self.directory.join("input.csv");
        fs::write(&path, text).unwrap();
        import_table_csv(&self.state, ImportCsvRequest {
            connection_id: self.id.clone(), database: "demo".into(), schema: self.schema.clone(), table: self.items.clone(), path: path.display().to_string(), truncate: false,
        })
    }
}

impl Drop for ServerFixture {
    fn drop(&mut self) {
        let _ = self.editor("ROLLBACK");
        if let Ok(mut session) = self.state.open_isolated_session(&self.id) {
            for table in [&self.items, &self.marker] { let _ = session.execute("demo", &format!("DROP TABLE IF EXISTS {table}"), None); }
        }
        self.state.disconnect(&self.id);
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn server_writes_are_isolated(engine: Engine) {
    let fixture = ServerFixture::new(engine);
    fixture.editor("BEGIN").unwrap();
    fixture.editor(&format!("UPDATE {} SET value=55 WHERE id=1", fixture.marker)).unwrap();
    fixture.editor(&format!("INSERT INTO {} VALUES (2,66)", fixture.marker)).unwrap();
    assert_eq!(fixture.grid(1).unwrap().applied, 1);
    assert_eq!(fixture.csv("id,value\n2,csv\n").unwrap().inserted, 1);
    assert!(fixture.grid(1).is_err());
    assert!(fixture.csv("id,value\n3,temporary\n1,conflict\n").is_err());
    let marker_query = format!("SELECT id,value FROM {} ORDER BY id", fixture.marker);
    let items_query = format!("SELECT id FROM {} ORDER BY id", fixture.items);
    // The editor owns an actual UPDATE and INSERT, yet other sessions see neither.
    assert_eq!(fixture.external(&marker_query), vec![vec![json!(1), json!(0)]]);
    assert_eq!(ServerFixture::numbers(fixture.editor(&marker_query).unwrap().rows), vec![vec![json!(1), json!(55)], vec![json!(2), json!(66)]]);
    assert_eq!(fixture.external(&items_query), vec![vec![json!(1)], vec![json!(2)]]);
    let read = crate::studio::execute_read_query_with_operation(&fixture.state, ExecuteSqlRequest {
        connection_id: fixture.id.clone(), database: "demo".into(), sql: marker_query.clone(), page: 1, page_size: 10, only_statement: None,
    }, &OperationContext::detached()).unwrap();
    assert_eq!(ServerFixture::numbers(read.statements[0].rows.clone()), vec![vec![json!(1), json!(0)]]);
    fixture.editor("ROLLBACK").unwrap();
    assert_eq!(fixture.external(&marker_query), vec![vec![json!(1), json!(0)]]);
    assert_eq!(fixture.external(&items_query), vec![vec![json!(1)], vec![json!(2)]]);
}

#[test]
#[ignore = "requires isolated MySQL on 127.0.0.1:13306 (compose.test.yml)"]
fn mysql_grid_csv_and_bi_never_finish_editor_transaction() {
    server_writes_are_isolated(Engine::Mysql);
}

#[test]
#[ignore = "requires isolated PostgreSQL on 127.0.0.1:15432 (compose.test.yml)"]
fn postgres_grid_csv_and_bi_never_finish_editor_transaction() {
    server_writes_are_isolated(Engine::Postgres);
}
