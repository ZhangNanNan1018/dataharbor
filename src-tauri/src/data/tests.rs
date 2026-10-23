use super::*;
use crate::models::{ExecuteSqlRequest, SaveConnectionRequest};
use crate::query::execute_sql;
use serde_json::json;
use uuid::Uuid;

#[test]
fn sqlite_pages_keep_user_sort_and_use_primary_key_tie_breaker() {
    let dir = std::env::temp_dir().join(format!("dataharbor-paging-{}", Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    {
        let state = AppState::open_in(dir.clone(), true).unwrap();
        let saved = state
            .save_connection(SaveConnectionRequest {
                name: "分页回归".into(),
                engine: Engine::Sqlite,
                file_path: dir.join("paging.sqlite").display().to_string(),
                ..Default::default()
            })
            .unwrap();
        let id = saved.connection.id;
        state.connect(&id).unwrap();
        let run = |sql: &str, page, size| {
            execute_sql(
                &state,
                ExecuteSqlRequest { session_id: None,
                    connection_id: id.clone(),
                    database: "main".into(),
                    sql: sql.into(),
                    page,
                    page_size: size,
                    only_statement: None,
                },
            )
            .unwrap()
        };
        let created = run("CREATE TABLE books (title TEXT, id INTEGER PRIMARY KEY, category TEXT); INSERT INTO books VALUES ('c',3,'same'),('a',1,'same'),('b',2,'same');", 1, 10);
        assert!(created.statements.iter().all(|item| item.ok));
        let load = |page| {
            fetch_rows(
                &state,
                FetchRowsRequest {
                    connection_id: id.clone(),
                    database: "main".into(),
                    schema: "main".into(),
                    table: "books".into(),
                    page,
                    page_size: 2,
                    filters: vec![],
                    sorts: vec![SortSpec {
                        column: "category".into(),
                        desc: false,
                    }],
                },
            )
            .unwrap()
        };
        let first = load(1);
        let second = load(2);
        assert_eq!(first.total, 3);
        assert_eq!(
            first
                .rows
                .iter()
                .map(|row| row[1].clone())
                .collect::<Vec<_>>(),
            vec![json!(1), json!(2)]
        );
        assert_eq!(second.rows[0][1], json!(3));
        assert!(first.columns[1].primary_key);

        let ordered = run(
            "WITH x AS (SELECT id FROM books) SELECT id FROM x ORDER BY id DESC",
            2,
            2,
        );
        let result = &ordered.statements[0];
        assert!(result.ok, "{:?}", result.error);
        assert!(result.pageable);
        assert_eq!(result.total_rows, Some(3));
        assert_eq!(result.rows, vec![vec![json!(1)]]);

        let first_query_page = run("SELECT id FROM books ORDER BY id", 1, 2);
        assert!(first_query_page.statements[0].has_more);
        assert_eq!(first_query_page.statements[0].total_rows, None);

        let explicit_limit = run("SELECT id FROM books ORDER BY id LIMIT 1", 1, 2);
        assert_eq!(explicit_limit.statements[0].rows.len(), 1);
        assert!(!explicit_limit.statements[0].pageable);
        assert!(!run("SELECT id FROM books LIMIT 1", 2, 2).statements[0].ok);
        state.disconnect(&id);
    }
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn exporting_a_written_result_cannot_replay_the_write() {
    let dir = std::env::temp_dir().join(format!("dataharbor-export-{}", Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    {
        let state = AppState::open_in(dir.clone(), true).unwrap();
        let saved = state
            .save_connection(SaveConnectionRequest {
                name: "导出回归".into(),
                engine: Engine::Sqlite,
                file_path: dir.join("counter.sqlite").display().to_string(),
                ..Default::default()
            })
            .unwrap();
        let id = saved.connection.id;
        state.connect(&id).unwrap();
        state
            .with_session(&id, |session| {
                session.execute(
                    "main",
                    "CREATE TABLE counter (id INTEGER PRIMARY KEY, n INTEGER)",
                    None,
                )?;
                session.execute("main", "INSERT INTO counter VALUES (1, 0)", None)
            })
            .unwrap();
        state
            .with_session(&id, |session| {
                session.execute("main", "UPDATE counter SET n=n+1 RETURNING n", None)
            })
            .unwrap();
        let path = dir.join("result.csv").display().to_string();
        for sql in [
            "UPDATE counter SET n=n+1 RETURNING n",
            "DELETE FROM counter",
            "WITH changed AS (DELETE FROM counter RETURNING *) SELECT * FROM changed",
        ] {
            assert!(export_sql_csv(
                &state,
                ExportSqlRequest { session_id: None,
                    connection_id: id.clone(),
                    database: "main".into(),
                    sql: sql.into(),
                    path: path.clone()
                }
            )
            .is_err());
        }
        assert!(!Path::new(&path).exists());
        let after = state
            .with_session(&id, |session| {
                session.execute("main", "SELECT n FROM counter", None)
            })
            .unwrap();
        assert_eq!(after.rows, vec![vec![json!(1)]]);
        let exported = export_sql_csv(
            &state,
            ExportSqlRequest { session_id: None,
                connection_id: id.clone(),
                database: "main".into(),
                sql: "SELECT n FROM counter LIMIT 1".into(),
                path,
            },
        )
        .unwrap();
        assert_eq!(exported.row_count, 1);
        assert!(!exported.truncated);
        state.disconnect(&id);
    }
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn sorting_appends_all_composite_keys_and_rejects_offset_overflow() {
    let mut tenant = ColumnMeta::named("tenant");
    tenant.primary_key = true;
    let mut id = ColumnMeta::named("id");
    id.primary_key = true;
    let sorts = stable_sort(
        &[SortSpec {
            column: "id".into(),
            desc: true,
        }],
        &[tenant, id],
    );
    assert_eq!(sorts.len(), 2);
    assert_eq!(sorts[0].column, "id");
    assert!(sorts[0].desc);
    assert_eq!(sorts[1].column, "tenant");
    assert!(page_offset(u32::MAX, 500).is_err());
}

struct RecordingSession {
    calls: Vec<String>,
    fail_on: Option<String>,
    cancel_after: Option<(String, OperationContext)>,
}

impl DbSession for RecordingSession {
    fn engine(&self) -> Engine {
        Engine::Mssql
    }
    fn version(&mut self) -> AppResult<String> {
        Ok(String::new())
    }
    fn list_databases(&mut self) -> AppResult<Vec<String>> {
        Ok(vec![])
    }
    fn list_schemas(&mut self, _: &str) -> AppResult<Vec<String>> {
        Ok(vec![])
    }
    fn list_tables(&mut self, _: &str, _: &str) -> AppResult<Vec<String>> {
        Ok(vec![])
    }
    fn list_views(&mut self, _: &str, _: &str) -> AppResult<Vec<String>> {
        Ok(vec![])
    }
    fn list_columns(&mut self, _: &str, _: &str, _: &str) -> AppResult<Vec<ColumnMeta>> {
        Ok(vec![])
    }
    fn relation_kind(&mut self, _: &str, _: &str, _: &str) -> AppResult<Option<String>> {
        Ok(Some("table".into()))
    }
    fn execute(
        &mut self,
        _: &str,
        sql: &str,
        _: Option<usize>,
    ) -> AppResult<crate::models::QueryOutcome> {
        self.calls.push(sql.into());
        if let Some((trigger, operation)) = &self.cancel_after {
            if trigger == sql { operation.request_cancel()?; }
        }
        if self.fail_on.as_deref() == Some(sql) {
            return Err(AppError::msg("fixture failure"));
        }
        Ok(crate::models::QueryOutcome::empty())
    }
}

#[test]
fn mssql_transaction_starts_with_begin_transaction_and_rolls_back_on_failure() {
    let mut session = RecordingSession {
        calls: vec![],
        fail_on: None,
        cancel_after: None,
    };
    session
        .execute_transaction("db", &["INSERT first".into()])
        .unwrap();
    assert_eq!(
        session.calls,
        ["BEGIN TRANSACTION", "INSERT first", "COMMIT"]
    );
    session.calls.clear();
    session.fail_on = Some("INSERT failed".into());
    assert!(session
        .execute_transaction("db", &["INSERT first".into(), "INSERT failed".into()])
        .is_err());
    assert_eq!(
        session.calls,
        [
            "BEGIN TRANSACTION",
            "INSERT first",
            "INSERT failed",
            "ROLLBACK"
        ]
    );
}

#[test]
fn sqlite_grid_and_csv_failure_preserve_the_entire_previous_state() {
    let dir = std::env::temp_dir().join(format!("dataharbor-transactions-{}", Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    {
        let state = AppState::open_in(dir.clone(), true).unwrap();
        let saved = state
            .save_connection(SaveConnectionRequest {
                name: "事务回归".into(),
                engine: Engine::Sqlite,
                file_path: dir.join("transactions.sqlite").display().to_string(),
                ..Default::default()
            })
            .unwrap();
        let id = saved.connection.id;
        state.connect(&id).unwrap();
        state
            .with_session(&id, |session| {
                session.execute(
                    "main",
                    "CREATE TABLE items (id INTEGER PRIMARY KEY, bytes BLOB)",
                    None,
                )
            })
            .unwrap();
        state
            .with_session(&id, |session| {
                session.execute("main", "INSERT INTO items VALUES (9, X'FF')", None)
            })
            .unwrap();
        let change = |id| RowChange {
            original: None,
            kind: "insert".into(),
            pk: vec![],
            values: vec![FieldValue {
                column: "id".into(),
                value: json!(id),
            }],
        };
        assert!(commit_changes(
            &state,
            CommitRequest {
                connection_id: id.clone(),
                database: "main".into(),
                schema: String::new(),
                table: "items".into(),
                changes: vec![change(1), change(9)]
            }
        )
        .is_err());
        let read = || {
            state
                .with_session(&id, |session| {
                    session.execute("main", "SELECT id, hex(bytes) FROM items ORDER BY id", None)
                })
                .unwrap()
                .rows
        };
        assert_eq!(read(), vec![vec![json!(9), json!("FF")]]);

        let path = dir.join("invalid.csv");
        fs::write(&path, "id,bytes\n1,base64:AA==\n2,invalid-binary\n").unwrap();
        let error = import_table_csv(
            &state,
            ImportCsvRequest {
                connection_id: id.clone(),
                database: "main".into(),
                schema: String::new(),
                table: "items".into(),
                path: path.display().to_string(),
                truncate: true,
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("第 3 行 CSV 无效"));
        assert_eq!(read(), vec![vec![json!(9), json!("FF")]]);

        fs::write(&path, "id,bytes\n1,base64:AA==\n1,base64:AQ==\n").unwrap();
        assert!(import_table_csv(
            &state,
            ImportCsvRequest {
                connection_id: id.clone(),
                database: "main".into(),
                schema: String::new(),
                table: "items".into(),
                path: path.display().to_string(),
                truncate: true
            }
        )
        .is_err());
        assert_eq!(read(), vec![vec![json!(9), json!("FF")]]);
        state.disconnect(&id);
    }
    fs::remove_dir_all(dir).unwrap();
}


struct EditFixture {
    state: AppState,
    id: String,
    dir: std::path::PathBuf,
}

impl EditFixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("dataharbor-edit-{}", Uuid::new_v4()));
        let state = AppState::open_in(dir.clone(), true).unwrap();
        let id = state.save_connection(SaveConnectionRequest {
            name: "编辑回归".into(), engine: Engine::Sqlite,
            file_path: dir.join("test.sqlite").display().to_string(), ..Default::default()
        }).unwrap().connection.id;
        state.connect(&id).unwrap();
        Self { state, id, dir }
    }
    fn sql(&self, sql: &str) -> Vec<Vec<JsonValue>> {
        self.state.with_session(&self.id, |session| session.execute("main", sql, None)).unwrap().rows
    }
    fn commit(&self, changes: Vec<RowChange>) -> AppResult<CommitResult> {
        commit_changes(&self.state, CommitRequest { connection_id: self.id.clone(), database: "main".into(), schema: "".into(), table: "items".into(), changes })
    }
}
impl Drop for EditFixture {
    fn drop(&mut self) {
        self.state.disconnect(&self.id);
        let _ = fs::remove_dir_all(&self.dir);
    }
}
fn edit(id: i64, before: JsonValue, after: JsonValue) -> RowChange {
    RowChange {
        kind: "update".into(),
        original: Some(vec![FieldValue { column: "id".into(), value: json!(id) }, FieldValue { column: "label".into(), value: before }]),
        pk: vec![FieldValue { column: "id".into(), value: json!(id) }],
        values: vec![FieldValue { column: "label".into(), value: after }],
    }
}

#[test]
fn concurrent_update_or_delete_rolls_back_the_entire_edit_batch() {
    let fixture = EditFixture::new();
    fixture.sql("CREATE TABLE items(id INTEGER PRIMARY KEY, label TEXT)");
    fixture.sql("INSERT INTO items VALUES(1,'first'),(2,'second'),(3,NULL)");
    let changes = vec![edit(1, json!("first"), json!("mine")), edit(2, json!("second"), json!("mine"))];
    // A separate connection simulates another client, not another local edit.
    let other = rusqlite::Connection::open(fixture.dir.join("test.sqlite")).unwrap();
    other.execute("UPDATE items SET label='theirs' WHERE id=2", []).unwrap();
    let error = fixture.commit(changes.clone()).unwrap_err().to_string();
    assert!(error.contains("数据已被修改或删除"), "{error}");
    assert_eq!(fixture.sql("SELECT label FROM items WHERE id=1"), vec![vec![json!("first")]]);
    other.execute("DELETE FROM items WHERE id=2", []).unwrap();
    assert!(fixture.commit(changes).is_err());
    assert_eq!(fixture.sql("SELECT label FROM items WHERE id=1"), vec![vec![json!("first")]]);
    assert_eq!(fixture.commit(vec![edit(3, JsonValue::Null, json!("now"))]).unwrap().applied, 1);
    assert_eq!(fixture.sql("SELECT label FROM items WHERE id=3"), vec![vec![json!("now")]]);
    let mut deletion = edit(1, json!("wrong"), JsonValue::Null);
    deletion.kind = "delete".into(); deletion.values.clear();
    assert!(fixture.commit(vec![deletion]).is_err());
    let mut missing = edit(1, json!("first"), json!("mine")); missing.original = None;
    assert!(fixture.commit(vec![missing]).is_err());
    drop(other);
}

#[test]
fn csv_routes_round_trip_quotes_newlines_null_empty_and_text_numbers() {
    let fixture = EditFixture::new();
    fixture.sql("CREATE TABLE items(id INTEGER PRIMARY KEY, label TEXT)");
    let values = [json!("00123"), json!("a\"b\n第二行"), json!(""), JsonValue::Null, json!("\\N"), json!("18446744073709551615.123456789")];
    let changes = values.iter().enumerate().map(|(i, value)| RowChange { kind: "insert".into(), original: None, pk: vec![], values: vec![FieldValue { column: "id".into(), value: json!(i+1) }, FieldValue { column: "label".into(), value: value.clone() }] }).collect();
    fixture.commit(changes).unwrap();
    let expected = fixture.sql("SELECT * FROM items ORDER BY id");
    for studio_export in [false, true] {
        let path = fixture.dir.join("roundtrip.csv").display().to_string();
        if studio_export {
            crate::studio::export_file(&fixture.state, crate::studio::FileExportRequest { connection_id: fixture.id.clone(), database: "main".into(), schema: "".into(), table: "items".into(), format: "csv".into(), path: path.clone() }).unwrap();
        } else {
            export_table_csv(&fixture.state, ExportTableRequest { connection_id: fixture.id.clone(), database: "main".into(), schema: "".into(), table: "items".into(), filters: vec![], sorts: vec![], path: path.clone() }).unwrap();
        }
        for studio_import in [false, true] {
            fixture.sql("DELETE FROM items");
            if studio_import {
                crate::studio::import_file(&fixture.state, crate::studio::FileImportRequest { connection_id: fixture.id.clone(), database: "main".into(), schema: "".into(), table: "items".into(), format: "csv".into(), path: path.clone() }).unwrap();
            } else {
                import_table_csv(&fixture.state, ImportCsvRequest { connection_id: fixture.id.clone(), database: "main".into(), schema: "".into(), table: "items".into(), path: path.clone(), truncate: false }).unwrap();
            }
            assert_eq!(fixture.sql("SELECT * FROM items ORDER BY id"), expected);
        }
    }
}

#[test]
fn numeric_literals_keep_decimal_precision_and_reject_sql_tokens() {
    use crate::sqlutil::value_sql;
    for kind in ["numeric(38,18)", "decimal(38,18)", "bigint unsigned"] {
        for text in ["18446744073709551615", "12345678901234567890.123456789012345678", "-1.2300e+12"] {
            assert_eq!(value_sql(Engine::Postgres, kind, &json!(text)).unwrap(), text);
        }
        for bad in ["", "NaN", "Infinity", "1e", "1; DELETE FROM items", "0 OR 1=1"] {
            assert!(value_sql(Engine::Postgres, kind, &json!(bad)).is_err());
        }
    }
}


#[test]
fn cancellation_rolls_back_current_transaction_without_executing_next_statement() {
    let operation = OperationContext::detached();
    let mut session = RecordingSession { calls: vec![], fail_on: None, cancel_after: Some(("INSERT first".into(), operation.clone())) };
    let error = session.execute_transaction_with_operation("db", &["INSERT first".into(), "INSERT second".into()], &operation).unwrap_err();
    assert!(matches!(error, AppError::Cancelled { committed: 0, batches: 0 }));
    assert_eq!(session.calls, ["BEGIN TRANSACTION", "INSERT first", "ROLLBACK"]);
}

#[test]
fn query_failure_and_uncommitted_writes_have_truthful_operation_state() {
    let fixture = EditFixture::new();
    fixture.sql("CREATE TABLE items(id INTEGER PRIMARY KEY, label TEXT)");
    fixture.sql("INSERT INTO items VALUES(1,'before')");
    let run = |sql: &str, operation: &OperationContext| {
        crate::query::execute_sql_with_operation(&fixture.state, ExecuteSqlRequest { session_id: None,
            connection_id: fixture.id.clone(), database: "main".into(), sql: sql.into(), page: 1, page_size: 10, only_statement: None,
        }, operation)
    };
    let transaction = OperationContext::detached();
    let result = run("BEGIN; UPDATE items SET label='uncommitted'; ROLLBACK", &transaction);
    transaction.finish(&result);
    assert!(!result.unwrap().stopped);
    assert_eq!(transaction.snapshot().committed, 0);
    assert_eq!(fixture.sql("SELECT label FROM items"), vec![vec![json!("before")]]);
    let failure = OperationContext::detached();
    let result = run("SELECT * FROM nonexistent; DELETE FROM items", &failure);
    failure.finish(&result);
    assert!(result.unwrap().stopped);
    assert_eq!(failure.snapshot().status, "failed");
    assert_eq!(fixture.sql("SELECT COUNT(*) FROM items"), vec![vec![json!(1)]]);
}

#[test]
fn cancelled_import_and_export_preserve_existing_data_and_output() {
    let fixture = EditFixture::new();
    fixture.sql("CREATE TABLE items(id INTEGER PRIMARY KEY, label TEXT)");
    fixture.sql("INSERT INTO items VALUES(1,'before')");
    let path = fixture.dir.join("protected.csv");
    fs::write(&path, "id,label\n2,after\n").unwrap();
    let operation = OperationContext::detached();
    operation.request_cancel().unwrap();
    let error = import_table_csv_with_operation(&fixture.state, ImportCsvRequest {
        connection_id: fixture.id.clone(), database: "main".into(), schema: "".into(), table: "items".into(), path: path.display().to_string(), truncate: true,
    }, &operation).unwrap_err();
    assert!(matches!(error, AppError::Cancelled { .. }));
    assert_eq!(fixture.sql("SELECT label FROM items"), vec![vec![json!("before")]]);
    assert!(write_csv_operation(path.to_str().unwrap(), &[ColumnMeta::named("x")], &[vec![json!("new")]], &operation).is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), "id,label\n2,after\n");
}
