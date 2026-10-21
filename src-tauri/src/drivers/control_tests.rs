//! Cancellation and consistent-read checks against isolated files/opt-in servers.
use super::{open_session, DbSession};
use crate::models::{Engine, ExecuteSqlRequest, SaveConnectionRequest, SessionConfig};
use crate::operations::OperationContext;
use crate::state::AppState;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn config(engine: Engine, path: &str, port: u16) -> SessionConfig {
    SessionConfig {
        engine,
        host: "127.0.0.1".into(),
        port,
        tls_server_name: None,
        username: if engine == Engine::Postgres {
            "postgres"
        } else {
            "root"
        }
        .into(),
        password: "dataharbor".into(),
        database: if engine == Engine::Sqlite {
            "main"
        } else {
            "demo"
        }
        .into(),
        file_path: path.into(),
        use_ssl: false,
        ssl_insecure: false,
        ssh_password: String::new(),
        server_mode: String::new(),
        auth_method: String::new(),
        warehouse: String::new(),
        schema_name: String::new(),
        address_map: String::new(),
    }
}

struct TempDirectory(PathBuf);
impl TempDirectory {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("dataharbor-control-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn count(session: &mut dyn DbSession, database: &str, table: &str) -> u64 {
    let result = session
        .execute(database, &format!("SELECT COUNT(*) FROM {table}"), Some(1))
        .unwrap();
    crate::sqlutil::json_count(&result.rows[0][0]).unwrap()
}

#[test]
fn sqlite_cancel_interrupts_a_running_query_and_connection_remains_usable() {
    let directory = TempDirectory::new();
    let cfg = config(
        Engine::Sqlite,
        directory.0.join("cancel.sqlite").to_str().unwrap(),
        0,
    );
    let mut session = open_session(&cfg).unwrap();
    let cancel = session.cancel_handle("main").unwrap().unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (finished_tx, finished_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        let result = session.execute("main", "WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<100000000) SELECT sum(x) FROM n", Some(1));
        finished_tx.send(result.is_err()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let next = session.execute("main", "SELECT 42", Some(1)).unwrap();
        assert_eq!(next.rows, vec![vec![json!(42)]]);
    });
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    // Reissue until an in-flight SQLite VM observes the interrupt. This avoids
    // assuming the worker has entered sqlite3_step immediately after signaling.
    let deadline = Instant::now() + Duration::from_secs(3);
    let interrupted = loop {
        cancel().unwrap();
        match finished_rx.recv_timeout(Duration::from_millis(10)) {
            Ok(interrupted) => break interrupted,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) if Instant::now() < deadline => {}
            Err(error) => panic!("query did not stop: {error}"),
        }
    };
    assert!(interrupted);
    release_tx.send(()).unwrap();
    worker.join().unwrap();
}

#[test]
fn sqlite_read_snapshot_is_stable_while_another_wal_connection_writes() {
    let directory = TempDirectory::new();
    let cfg = config(
        Engine::Sqlite,
        directory.0.join("snapshot.sqlite").to_str().unwrap(),
        0,
    );
    let mut writer = open_session(&cfg).unwrap();
    writer
        .execute("main", "PRAGMA journal_mode=WAL", None)
        .unwrap();
    writer
        .execute("main", "CREATE TABLE items(id INTEGER PRIMARY KEY)", None)
        .unwrap();
    writer
        .execute("main", "INSERT INTO items VALUES(1)", None)
        .unwrap();
    let mut reader = open_session(&cfg).unwrap();
    reader.begin_read_snapshot("main").unwrap();
    assert_eq!(count(reader.as_mut(), "main", "items"), 1);
    assert!(reader
        .execute("main", "INSERT INTO items VALUES(9)", None)
        .is_err());
    writer
        .execute("main", "INSERT INTO items VALUES(2)", None)
        .unwrap();
    assert_eq!(count(reader.as_mut(), "main", "items"), 1);
    reader.end_read_snapshot("main").unwrap();
    assert_eq!(count(reader.as_mut(), "main", "items"), 2);
}

fn server_config(engine: Engine) -> SessionConfig {
    let variable = if engine == Engine::Postgres {
        "DATAHARBOR_TEST_POSTGRES_PORT"
    } else {
        "DATAHARBOR_TEST_MYSQL_PORT"
    };
    let port = std::env::var(variable)
        .expect("需要专用本机测试端口")
        .parse::<u16>()
        .unwrap();
    config(engine, "", port)
}

fn wait_until_running(cfg: &SessionConfig, marker: &str) {
    let mut observer = open_session(cfg).unwrap();
    let sql = if cfg.engine == Engine::Postgres {
        format!("SELECT COUNT(*) FROM pg_stat_activity WHERE pid <> pg_backend_pid() AND state='active' AND query LIKE '%{marker}%'")
    } else {
        format!("SELECT COUNT(*) FROM information_schema.PROCESSLIST WHERE ID <> CONNECTION_ID() AND INFO LIKE '%{marker}%'")
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let result = observer.execute(&cfg.database, &sql, Some(1)).unwrap();
        if crate::sqlutil::json_count(&result.rows[0][0]).unwrap() > 0 {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "fixture query did not reach the server"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn server_snapshot_round_trip(engine: Engine) {
    let cfg = server_config(engine);
    let table = format!("dh_snapshot_{}", uuid::Uuid::new_v4().simple());
    let mut writer = open_session(&cfg).unwrap();
    writer
        .execute(
            "demo",
            &format!("CREATE TABLE {table}(id INTEGER PRIMARY KEY)"),
            None,
        )
        .unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        writer
            .execute("demo", &format!("INSERT INTO {table} VALUES(1)"), None)
            .unwrap();
        let mut reader = open_session(&cfg).unwrap();
        reader.begin_read_snapshot("demo").unwrap();
        reader
            .validate_snapshot_relation("demo", "public", &table)
            .unwrap();
        assert_eq!(count(reader.as_mut(), "demo", &table), 1);
        writer
            .execute("demo", &format!("INSERT INTO {table} VALUES(2)"), None)
            .unwrap();
        assert_eq!(count(reader.as_mut(), "demo", &table), 1);
        reader.end_read_snapshot("demo").unwrap();
        assert_eq!(count(reader.as_mut(), "demo", &table), 2);
    }));
    writer
        .execute("demo", &format!("DROP TABLE {table}"), None)
        .unwrap();
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}

#[test]
#[ignore = "需要专用本地 PostgreSQL 测试容器"]
fn postgres_snapshot_keeps_rows_stable_during_external_writes() {
    server_snapshot_round_trip(Engine::Postgres);
}

#[test]
#[ignore = "需要专用本地 MySQL 测试容器"]
fn mysql_snapshot_keeps_rows_stable_and_refuses_nontransactional_tables() {
    server_snapshot_round_trip(Engine::Mysql);
    let cfg = server_config(Engine::Mysql);
    let table = format!("dh_myisam_{}", uuid::Uuid::new_v4().simple());
    let mut session = open_session(&cfg).unwrap();
    session
        .execute(
            "demo",
            &format!("CREATE TABLE {table}(id INTEGER PRIMARY KEY) ENGINE=MyISAM"),
            None,
        )
        .unwrap();
    session.begin_read_snapshot("demo").unwrap();
    let result = session.validate_snapshot_relation("demo", "", &table);
    session.end_read_snapshot("demo").unwrap();
    session
        .execute("demo", &format!("DROP TABLE {table}"), None)
        .unwrap();
    assert!(result.unwrap_err().to_string().contains("InnoDB"));
}

#[test]
#[ignore = "需要专用本地 MySQL 测试容器"]
fn mysql_cancel_uses_an_independent_connection_and_keeps_session_usable() {
    let cfg = server_config(Engine::Mysql);
    let marker = format!("dh_cancel_{}", uuid::Uuid::new_v4().simple());
    let mut session = open_session(&cfg).unwrap();
    let cancel = session.cancel_handle("demo").unwrap().unwrap();
    let query = format!("SELECT SLEEP(10) /*{marker}*/");
    let worker = std::thread::spawn(move || {
        let started = Instant::now();
        let result = session.execute("demo", &query, Some(1));
        let elapsed = started.elapsed();
        let next = session.execute("demo", "SELECT 42", Some(1)).unwrap();
        assert_eq!(crate::sqlutil::json_count(&next.rows[0][0]), Some(42));
        (result, elapsed)
    });
    wait_until_running(&cfg, &marker);
    cancel().unwrap();
    let (result, elapsed) = worker.join().unwrap();
    // MySQL's standalone SLEEP returns 1 when KILL QUERY interrupts it. A
    // normally completed SLEEP(10) returns 0, so merely accepting Ok is unsafe.
    let interrupted = result.expect("standalone SLEEP should report its interruption value");
    assert_eq!(crate::sqlutil::json_count(&interrupted.rows[0][0]), Some(1));
    assert!(
        elapsed < Duration::from_secs(8),
        "SLEEP did not stop early: {elapsed:?}"
    );
}

#[test]
#[ignore = "需要专用本地 PostgreSQL 测试容器"]
fn postgres_cancel_stops_later_statements_without_replaying_writes() {
    cancel_stops_later_statements_without_replaying_writes(Engine::Postgres);
}

#[test]
#[ignore = "需要专用本地 MySQL 测试容器"]
fn mysql_cancel_stops_later_statements_even_when_sleep_returns_success() {
    cancel_stops_later_statements_without_replaying_writes(Engine::Mysql);
}

fn cancel_stops_later_statements_without_replaying_writes(engine: Engine) {
    let directory = TempDirectory::new();
    let cfg = server_config(engine);
    let state = Arc::new(AppState::open_in(directory.0.clone(), true).unwrap());
    let id = state
        .save_connection(SaveConnectionRequest {
            name: "取消回归".into(),
            engine,
            host: cfg.host.clone(),
            port: cfg.port,
            username: cfg.username.clone(),
            password: Some(cfg.password.clone()),
            database: "demo".into(),
            ..Default::default()
        })
        .unwrap()
        .connection
        .id;
    state.connect(&id).unwrap();
    let table = format!("dh_cancel_{}", uuid::Uuid::new_v4().simple());
    state
        .with_session(&id, |session| {
            session.execute(
                "demo",
                &format!("CREATE TABLE {table}(id INTEGER PRIMARY KEY)"),
                None,
            )
        })
        .unwrap();
    let operation = OperationContext::detached();
    let worker_state = state.clone();
    let worker_op = operation.clone();
    let worker_id = id.clone();
    let marker = format!("dh_running_{}", uuid::Uuid::new_v4().simple());
    let sleeper = if engine == Engine::Postgres {
        "pg_sleep"
    } else {
        "SLEEP"
    };
    let query = format!("SELECT {sleeper}(10) /*{marker}*/; INSERT INTO {table} VALUES(1)");
    let worker = std::thread::spawn(move || {
        crate::query::execute_sql_with_operation(
            &worker_state,
            ExecuteSqlRequest {
                connection_id: worker_id,
                database: "demo".into(),
                sql: query,
                page: 1,
                page_size: 20,
                only_statement: None,
            },
            &worker_op,
        )
    });
    wait_until_running(&cfg, &marker);
    assert!(operation.snapshot().interruptible);
    operation.request_cancel().unwrap();
    let result = worker.join().unwrap();
    let rows = state
        .with_session(&id, |session| {
            session.execute("demo", &format!("SELECT COUNT(*) FROM {table}"), Some(1))
        })
        .unwrap();
    state
        .with_session(&id, |session| {
            session.execute("demo", &format!("DROP TABLE {table}"), None)
        })
        .unwrap();
    state.disconnect(&id);
    assert!(matches!(
        result,
        Err(crate::error::AppError::Cancelled { .. })
    ));
    assert_eq!(crate::sqlutil::json_count(&rows.rows[0][0]), Some(0));
}
