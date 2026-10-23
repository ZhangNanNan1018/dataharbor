use super::*;
use crate::models::{Engine, ExportSqlRequest, SaveConnectionRequest};
use serde_json::json;
use std::{fs, path::PathBuf};

struct Fixture {
    state: AppState,
    id: String,
    dir: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let dir =
            std::env::temp_dir().join(format!("dataharbor-consoles-{}", uuid::Uuid::new_v4()));
        let state = AppState::open_in(dir.clone(), true).unwrap();
        let id = state
            .save_connection(SaveConnectionRequest {
                name: "console fixture".into(),
                engine: Engine::Sqlite,
                file_path: dir.join("fixture.sqlite").display().to_string(),
                ..Default::default()
            })
            .unwrap()
            .connection
            .id;
        state.connect(&id).unwrap();
        let fixture = Self { state, id, dir };
        fixture.run(None, "PRAGMA journal_mode=WAL; CREATE TABLE marker(value INTEGER); INSERT INTO marker VALUES(0)");
        fixture
    }
    fn run(&self, console: Option<&str>, sql: &str) -> ExecuteSqlResponse {
        execute_sql(
            &self.state,
            ExecuteSqlRequest {
                connection_id: self.id.clone(),
                session_id: console.map(str::to_string),
                database: "main".into(),
                sql: sql.into(),
                page: 1,
                page_size: 20,
                only_statement: None,
            },
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.state.disconnect(&self.id);
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn consoles_and_ephemeral_jobs_cannot_commit_each_others_transactions() {
    let f = Fixture::new();
    assert!(!f.run(Some("A"), "BEGIN; UPDATE marker SET value=7").stopped);
    assert_eq!(
        f.run(Some("B"), "SELECT value FROM marker").statements[0].rows,
        vec![vec![json!(0)]]
    );
    assert!(f.run(Some("B"), "COMMIT").stopped);
    assert!(f.run(None, "COMMIT").stopped);
    assert_eq!(
        f.run(Some("A"), "SELECT value FROM marker").statements[0].rows,
        vec![vec![json!(7)]]
    );
    assert!(!f.run(Some("A"), "COMMIT").stopped);
    assert_eq!(
        f.run(None, "SELECT value FROM marker").statements[0].rows,
        vec![vec![json!(7)]]
    );
}

#[test]
fn temporary_objects_and_export_keep_console_identity_and_close_rolls_back() {
    let f = Fixture::new();
    f.run(
        Some("A"),
        "CREATE TEMP TABLE local_value(value TEXT); INSERT INTO local_value VALUES('mine')",
    );
    assert!(f.run(Some("B"), "SELECT * FROM local_value").stopped);
    let out = f.dir.join("console.csv");
    let exported = crate::data::export_sql_csv(
        &f.state,
        ExportSqlRequest {
            session_id: Some("A".into()),
            connection_id: f.id.clone(),
            database: "main".into(),
            sql: "SELECT * FROM local_value".into(),
            path: out.display().to_string(),
        },
    )
    .unwrap();
    assert_eq!(exported.row_count, 1);
    assert!(fs::read_to_string(out).unwrap().contains("mine"));
    f.run(Some("A"), "BEGIN; UPDATE marker SET value=8");
    f.state.close_query_session("A");
    assert_eq!(
        f.run(None, "SELECT value FROM marker").statements[0].rows,
        vec![vec![json!(0)]]
    );
    assert!(f.run(Some("A"), "SELECT * FROM local_value").stopped);
}

#[test]
fn scheduled_style_invocations_can_write_while_console_keeps_its_read_snapshot() {
    let f = Fixture::new();
    f.run(Some("A"), "BEGIN; SELECT * FROM marker");
    assert!(!f.run(None, "UPDATE marker SET value=9").stopped);
    assert_eq!(
        f.run(Some("A"), "SELECT value FROM marker").statements[0].rows,
        vec![vec![json!(0)]]
    );
    f.run(Some("A"), "ROLLBACK");
    assert_eq!(
        f.run(Some("A"), "SELECT value FROM marker").statements[0].rows,
        vec![vec![json!(9)]]
    );
}

#[test]
fn console_identity_cannot_be_reused_for_another_database() {
    let f = Fixture::new();
    f.run(Some("A"), "SELECT 1");
    let result = execute_sql(
        &f.state,
        ExecuteSqlRequest {
            session_id: Some("A".into()),
            connection_id: f.id.clone(),
            database: "other".into(),
            sql: "SELECT 1".into(),
            page: 1,
            page_size: 10,
            only_statement: None,
        },
    );
    assert!(result.unwrap_err().to_string().contains("连接已变化"));
}

#[test]
fn closing_console_rejects_an_already_queued_script() {
    let f = Fixture::new();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let fixture = &f;
        let first = scope.spawn(move || {
            fixture.state.with_query_session(
                &fixture.id,
                "main",
                Some("A"),
                &OperationContext::detached(),
                |_| {
                    entered_tx.send(()).unwrap();
                    release_rx
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .unwrap();
                    Ok(())
                },
            )
        });
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let operation = OperationContext::detached();
        let worker = operation.clone();
        let fixture = &f;
        let queued = scope.spawn(move || {
            fixture
                .state
                .with_query_session(&fixture.id, "main", Some("A"), &worker, |_| {
                    panic!("retired console must not execute queued SQL")
                })
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while operation.snapshot().phase != "等待查询会话" {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        f.state.close_query_session("A");
        release_tx.send(()).unwrap();
        first.join().unwrap().unwrap();
        let result: AppResult<()> = queued.join().unwrap();
        assert!(result.unwrap_err().to_string().contains("排队的语句未执行"));
    });
}

#[test]
#[ignore = "requires isolated PostgreSQL from compose.test.yml"]
fn postgres_oversized_page_is_an_error_instead_of_skipping_undisplayed_rows() {
    let dir = std::env::temp_dir().join(format!("dataharbor-page-budget-{}", uuid::Uuid::new_v4()));
    let state = AppState::open_in(dir.clone(), true).unwrap();
    let id = state
        .save_connection(SaveConnectionRequest {
            name: "page budget fixture".into(),
            engine: Engine::Postgres,
            host: "127.0.0.1".into(),
            port: std::env::var("DATAHARBOR_TEST_POSTGRES_PORT")
                .unwrap()
                .parse()
                .unwrap(),
            username: "postgres".into(),
            password: Some("dataharbor".into()),
            database: "demo".into(),
            ..Default::default()
        })
        .unwrap()
        .connection
        .id;
    state.connect(&id).unwrap();
    let request = ExecuteSqlRequest {
        session_id: Some("budget".into()),
        connection_id: id.clone(),
        database: "demo".into(),
        sql: "SELECT repeat('x',1048576) FROM generate_series(1,64)".into(),
        page: 1,
        page_size: 1000,
        only_statement: None,
    };
    let result = execute_sql(&state, request).unwrap();
    assert!(result.stopped);
    assert!(result.statements[0]
        .error
        .as_ref()
        .unwrap()
        .contains("分页已停止"));
    state.disconnect(&id);
    drop(state);
    fs::remove_dir_all(dir).unwrap();
}
