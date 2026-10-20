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
                ExecuteSqlRequest {
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
                ExportSqlRequest {
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
            ExportSqlRequest {
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
