use super::*;
use crate::error::DisconnectPhase;
use crate::models::{
    ColumnMeta, Engine, ExecuteSqlRequest, ExportSqlRequest, QueryOutcome, SaveConnectionRequest,
};

struct Wire {
    sent: Arc<Mutex<Vec<String>>>,
    fail_on: String,
    before_send: bool,
    probe_error: Option<AppError>,
}

impl DbSession for Wire {
    fn engine(&self) -> Engine {
        Engine::Mysql
    }
    fn probe(&mut self, _: &str) -> AppResult<()> {
        self.probe_error.take().map_or(Ok(()), Err)
    }

    fn version(&mut self) -> AppResult<String> {
        unreachable!()
    }
    fn list_databases(&mut self) -> AppResult<Vec<String>> {
        unreachable!()
    }
    fn list_schemas(&mut self, _: &str) -> AppResult<Vec<String>> {
        unreachable!()
    }
    fn list_tables(&mut self, _: &str, _: &str) -> AppResult<Vec<String>> {
        unreachable!()
    }
    fn list_views(&mut self, _: &str, _: &str) -> AppResult<Vec<String>> {
        unreachable!()
    }
    fn list_columns(&mut self, _: &str, _: &str, _: &str) -> AppResult<Vec<ColumnMeta>> {
        unreachable!()
    }
    fn relation_kind(&mut self, _: &str, _: &str, _: &str) -> AppResult<Option<String>> {
        unreachable!()
    }
    fn cancel_handle(&mut self, _: &str) -> AppResult<Option<crate::operations::CancelHandle>> {
        if self.before_send {
            return Err(AppError::driver("socket closed during setup", true));
        }
        Ok(None)
    }
    fn execute(&mut self, _: &str, sql: &str, _: Option<usize>) -> AppResult<QueryOutcome> {
        lock(&self.sent).push(sql.into());
        if sql.contains(&self.fail_on) {
            return Err(AppError::driver("response lost", true));
        }
        Ok(QueryOutcome::empty())
    }
}

#[test]
fn console_preflight_preserves_healthy_sessions_and_only_replaces_the_dead_generation() {
    let dir = std::env::temp_dir().join(format!("dataharbor-console-{}", uuid::Uuid::new_v4()));
    let state = AppState::open_in(dir.clone(), true).unwrap();
    let id = state
        .save_connection(SaveConnectionRequest {
            engine: Engine::Sqlite,
            file_path: dir.join("fixture.db").display().to_string(),
            name: "preflight".into(),
            ..Default::default()
        })
        .unwrap()
        .connection
        .id;
    state.connect(&id).unwrap();
    let prepare = |key: Option<&str>| state.prepare_query_session(&id, "", key, true);
    let a = prepare(None).unwrap();
    let b = prepare(None).unwrap();
    let sql = |key: &str, text: &str| {
        state.with_query_session(&id, "", Some(key), &OperationContext::detached(), |s| {
            s.execute("", text, None)
        })
    };
    sql(&b, "CREATE TEMP TABLE marker(n INTEGER)").unwrap();
    sql(&b, "BEGIN").unwrap();
    sql(&b, "INSERT INTO marker VALUES (7)").unwrap();
    sql(&a, "CREATE TABLE counter(n INTEGER)").unwrap();
    let sent = Arc::new(Mutex::new(Vec::new()));
    let old = lock(&state.query_sessions.0)[&a].clone();
    let install = |error| {
        lock(&old.session).as_mut().unwrap().inner = Box::new(Wire {
            sent: sent.clone(),
            fail_on: "unused".into(),
            before_send: false,
            probe_error: Some(error),
        });
    };
    install(AppError::msg("probe permission denied"));
    assert!(prepare(Some(&a)).is_err());
    assert_eq!(prepare(Some(&a)).unwrap(), a);
    install(AppError::driver("socket closed", true));
    let next = prepare(Some(&a)).unwrap();
    assert_ne!(a, next);
    assert!(sql(&a, "INSERT INTO counter VALUES (1)").is_err());
    sql(&next, "INSERT INTO counter VALUES (1)").unwrap();
    assert_eq!(
        sql(&next, "SELECT COUNT(*) FROM counter").unwrap().rows[0][0],
        serde_json::json!(1)
    );
    assert!(lock(&sent).is_empty());
    assert_eq!(prepare(Some(&b)).unwrap(), b);
    assert_eq!(
        sql(&b, "SELECT n FROM marker").unwrap().rows[0][0],
        serde_json::json!(7)
    );
    sql(&b, "ROLLBACK").unwrap();
    drop(old);
    state.disconnect(&id);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn lost_responses_never_replay_sql_paging_export_or_script_tail() {
    let dir = std::env::temp_dir().join(format!("dataharbor-wire-{}", uuid::Uuid::new_v4()));
    let state = AppState::open_in(dir.clone(), true).unwrap();
    let id = state
        .save_connection(SaveConnectionRequest {
            engine: Engine::Sqlite,
            file_path: dir.join("fixture.db").display().to_string(),
            name: "wire".into(),
            ..Default::default()
        })
        .unwrap()
        .connection
        .id;
    state.connect(&id).unwrap();
    for (script, fail_on, page, before_send, export) in [
        ("INSERT INTO events VALUES (1)", "INSERT", 1, false, false),
        ("COMMIT", "COMMIT", 1, false, false),
        ("SELECT tick()", "tick()", 1, false, false),
        ("SELECT tick()", "tick()", 2, false, false),
        ("SELECT tick()", "tick()", 1, false, true),
        (
            "SELECT 1; INSERT INTO events VALUES (2); SELECT 3",
            "INSERT",
            1,
            false,
            false,
        ),
        ("SELECT tick()", "tick()", 1, true, false),
    ] {
        let key = state.reconnect_query_session(&id, "", None).unwrap();
        let sent = Arc::new(Mutex::new(Vec::new()));
        let console = lock(&state.query_sessions.0)[&key].clone();
        lock(&console.session).as_mut().unwrap().inner = Box::new(Wire {
            sent: sent.clone(),
            before_send,
            probe_error: None,
            fail_on: fail_on.into(),
        });
        let expected = if before_send {
            DisconnectPhase::BeforeSend
        } else {
            DisconnectPhase::OutcomeUnknown
        };
        if export {
            let path = dir.join("result.csv");
            let error = crate::data::export_sql_csv(
                &state,
                ExportSqlRequest {
                    connection_id: id.clone(),
                    database: "".into(),
                    session_id: Some(key.clone()),
                    sql: script.into(),
                    path: path.display().to_string(),
                },
            )
            .unwrap_err();
            assert_eq!(error.disconnect_phase(), Some(expected));
            assert!(!path.exists());
        } else {
            let result = crate::query::execute_sql(
                &state,
                ExecuteSqlRequest {
                    connection_id: id.clone(),
                    database: "".into(),
                    session_id: Some(key.clone()),
                    page,
                    page_size: 10,
                    only_statement: None,
                    sql: script.into(),
                },
            )
            .unwrap();
            assert!(result.stopped);
            assert_eq!(
                result.statements.last().unwrap().disconnect_phase,
                Some(expected)
            );
            if script.contains(';') {
                assert!(result.statements[0].ok);
                assert_eq!(result.statements.len(), 2);
                assert_eq!(result.statements[1].index, 1);
            }
        }
        assert_eq!(
            lock(&sent)
                .iter()
                .filter(|sql| sql.contains(fail_on))
                .count(),
            usize::from(!before_send)
        );
        assert!(!lock(&sent).iter().any(|sql| sql.contains("SELECT 3")));
        let queued = state.with_query_session(
            &id,
            "",
            Some(&key),
            &OperationContext::detached(),
            |_| -> AppResult<()> { panic!("broken generation dispatched queued SQL") },
        );
        assert_eq!(
            queued.unwrap_err().disconnect_phase(),
            Some(DisconnectPhase::BeforeSend)
        );
        state.close_query_session(&key);
        assert!(state.reconnect_query_session(&id, "", Some(&key)).is_err());
    }
    state.disconnect(&id);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn protocol_failures_restore_shared_reads_only_before_dispatch() {
    let dir = std::env::temp_dir().join(format!("dataharbor-probes-{}", uuid::Uuid::new_v4()));
    let state = AppState::open_in(dir.clone(), true).unwrap();
    let id = state
        .save_connection(SaveConnectionRequest {
            engine: Engine::Sqlite,
            file_path: dir.join("fixture.db").display().to_string(),
            name: "probes".into(),
            ..Default::default()
        })
        .unwrap()
        .connection
        .id;
    state.connect(&id).unwrap();
    let slot = state.session_owner(&id).unwrap();
    let sent = Arc::new(Mutex::new(Vec::new()));
    let install = |probe_error| {
        lock(&slot.inner).inner = Box::new(Wire {
            sent: sent.clone(),
            before_send: false,
            probe_error,
            fail_on: "tick".into(),
        });
    };
    for e in crate::drivers::disconnect_tests::lost_response_fixtures() {
        install(Some(e));
        let row = state
            .with_read_session(&id, "", |s| s.execute("", "SELECT 42", None))
            .unwrap();
        assert_eq!(row.rows[0][0], serde_json::json!(42));
        assert!(
            lock(&sent).is_empty(),
            "broken session received business SQL"
        );
    }
    install(Some(AppError::msg("permission denied")));
    assert!(state
        .with_read_session(&id, "", |_| -> AppResult<()> {
            panic!("probe failure dispatched SQL")
        })
        .is_err());
    assert!(state
        .with_read_session(&id, "", |s| s.execute("", "SELECT tick()", None))
        .is_err());
    assert_eq!(*lock(&sent), ["SELECT tick()"]);
    assert_eq!(
        state
            .with_read_session(&id, "", |s| s.execute("", "SELECT 1", None))
            .unwrap()
            .rows[0][0],
        serde_json::json!(1)
    );
    assert_eq!(lock(&sent).len(), 1);
    drop(slot);
    state.disconnect(&id);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
#[ignore = "requires disposable MySQL at DATAHARBOR_TEST_MYSQL_PORT"]
fn replacing_http_tunnel_keeps_existing_console_on_its_original_tunnel() {
    use std::io::{BufRead, BufReader, Write};
    use std::net::{Shutdown, TcpListener, TcpStream};

    let db_port: u16 = std::env::var("DATAHARBOR_TEST_MYSQL_PORT")
        .unwrap()
        .parse()
        .unwrap();
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let proxy_port = listener.local_addr().unwrap().port();
    let reject_next = Arc::new(AtomicBool::new(false));
    let stopping = Arc::new(AtomicBool::new(false));
    let reject = reject_next.clone();
    let stop = stopping.clone();
    let proxy = std::thread::spawn(move || {
        let mut workers = Vec::new();
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            if stop.load(Ordering::Acquire) {
                break;
            }
            let reject = reject.swap(false, Ordering::AcqRel);
            workers.push(std::thread::spawn(move || {
                for line in BufReader::new(&stream).lines() {
                    if line.unwrap().is_empty() {
                        break;
                    }
                }
                if reject {
                    stream
                        .write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n")
                        .unwrap();
                    return;
                }
                let mut db = TcpStream::connect(("127.0.0.1", db_port)).unwrap();
                stream
                    .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                    .unwrap();
                let mut db_writer = db.try_clone().unwrap();
                let mut client_reader = stream.try_clone().unwrap();
                std::thread::scope(|s| {
                    s.spawn(move || {
                        let _ = std::io::copy(&mut client_reader, &mut db_writer);
                        let _ = db_writer.shutdown(Shutdown::Write);
                    });
                    let _ = std::io::copy(&mut db, &mut stream);
                    let _ = stream.shutdown(Shutdown::Write);
                });
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }
    });

    let dir = std::env::temp_dir().join(format!("dataharbor-tunnel-{}", uuid::Uuid::new_v4()));
    let state = AppState::open_in(dir.clone(), true).unwrap();
    let id = state
        .save_connection(SaveConnectionRequest {
            engine: Engine::Mysql,
            name: "http fixture".into(),
            host: "127.0.0.1".into(),
            port: db_port,
            username: "root".into(),
            password: Some("dataharbor".into()),
            database: "demo".into(),
            http_tunnel: true,
            http_proxy_host: "127.0.0.1".into(),
            http_proxy_port: proxy_port,
            ..Default::default()
        })
        .unwrap()
        .connection
        .id;
    state.connect(&id).unwrap();
    let original_port = state.live_transport(&id).unwrap().live.port;
    let shared_id = state
        .with_session(&id, |s| {
            Ok(s.execute("demo", "SELECT CONNECTION_ID()", None)?.rows[0][0].clone())
        })
        .unwrap();
    let key = state.reconnect_query_session(&id, "demo", None).unwrap();
    let exec = |sql: &str| {
        state
            .with_query_session(
                &id,
                "demo",
                Some(&key),
                &OperationContext::detached(),
                |s| s.execute("demo", sql, None),
            )
            .unwrap()
    };
    exec("SET @tunnel_marker = 'retained'");
    exec(&format!(
        "KILL CONNECTION {}",
        shared_id.to_string().trim_matches('"')
    ));
    reject_next.store(true, Ordering::Release);
    crate::schema::list_databases(&state, &id).unwrap();
    assert_ne!(state.live_transport(&id).unwrap().live.port, original_port);
    assert_eq!(
        exec("SELECT @tunnel_marker").rows[0][0],
        serde_json::json!("retained")
    );
    state.disconnect(&id);
    stopping.store(true, Ordering::Release);
    drop(TcpStream::connect(("127.0.0.1", proxy_port)).unwrap());
    proxy.join().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
