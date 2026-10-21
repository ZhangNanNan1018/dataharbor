use super::*;
use crate::models::{QueryOutcome, SaveConnectionRequest};
use crate::studio::backup::backup_sql;
use crate::studio::file_io::{export_file, export_streaming_file_with_hook};
use crate::studio::types::{BackupRequest, FileExportRequest};
use rusqlite::Connection;
use serde_json::json;
use std::fs;
use std::path::PathBuf;

struct Fixture {
    state: AppState,
    directory: PathBuf,
    id: String,
    database: PathBuf,
}
impl Fixture {
    fn new(sql: &str) -> Self {
        let directory =
            std::env::temp_dir().join(format!("dataharbor-read-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let database = directory.join("source.sqlite");
        Connection::open(&database)
            .unwrap()
            .execute_batch(&format!("PRAGMA journal_mode=WAL; {sql}"))
            .unwrap();
        let state = AppState::open_in(directory.clone(), true).unwrap();
        let saved = state
            .save_connection(SaveConnectionRequest {
                name: "read".into(),
                engine: Engine::Sqlite,
                file_path: database.display().to_string(),
                ..SaveConnectionRequest::default()
            })
            .unwrap();
        let id = saved.connection.id;
        state.connect(&id).unwrap();
        Self {
            state,
            directory,
            id,
            database,
        }
    }
    fn request(&self, format: &str) -> FileExportRequest {
        FileExportRequest {
            connection_id: self.id.clone(),
            database: "main".into(),
            schema: String::new(),
            table: "items".into(),
            format: format.into(),
            path: self
                .directory
                .join(format!("output.{format}"))
                .display()
                .to_string(),
        }
    }
    fn backup(&self) -> BackupRequest {
        BackupRequest {
            connection_id: self.id.clone(),
            database: "main".into(),
            schema: String::new(),
            path: self.directory.join("backup.sql").display().to_string(),
            mode: "builtin".into(),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.state.disconnect(&self.id);
        let _ = fs::remove_dir_all(&self.directory);
    }
}

struct Counted<'a> {
    inner: &'a mut dyn DbSession,
    metadata: usize,
    counts: usize,
    reads: usize,
    largest: usize,
    offsets: usize,
}
impl DbSession for Counted<'_> {
    fn engine(&self) -> Engine {
        self.inner.engine()
    }
    fn version(&mut self) -> AppResult<String> {
        self.inner.version()
    }
    fn list_databases(&mut self) -> AppResult<Vec<String>> {
        self.inner.list_databases()
    }
    fn list_schemas(&mut self, db: &str) -> AppResult<Vec<String>> {
        self.inner.list_schemas(db)
    }
    fn list_tables(&mut self, db: &str, schema: &str) -> AppResult<Vec<String>> {
        self.inner.list_tables(db, schema)
    }
    fn list_views(&mut self, db: &str, schema: &str) -> AppResult<Vec<String>> {
        self.inner.list_views(db, schema)
    }
    fn list_columns(&mut self, db: &str, schema: &str, name: &str) -> AppResult<Vec<ColumnMeta>> {
        self.metadata += 1;
        self.inner.list_columns(db, schema, name)
    }
    fn relation_kind(&mut self, db: &str, schema: &str, name: &str) -> AppResult<Option<String>> {
        self.inner.relation_kind(db, schema, name)
    }
    fn execute(&mut self, db: &str, sql: &str, max: Option<usize>) -> AppResult<QueryOutcome> {
        if sql.starts_with("SELECT COUNT(*)") {
            self.counts += 1;
        } else if sql.starts_with("SELECT") {
            self.reads += 1;
            self.largest = self.largest.max(max.unwrap_or(usize::MAX));
            self.offsets += usize::from(sql.contains("OFFSET"));
        }
        self.inner.execute(db, sql, max)
    }
}

#[test]
fn keyset_scan_reuses_count_and_metadata_and_bounds_every_application_batch() {
    let fixture = Fixture::new("CREATE TABLE items(id INTEGER PRIMARY KEY, payload TEXT); WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<200003) INSERT INTO items SELECT x, 'row-' || x FROM n;");
    let operation = OperationContext::detached();
    let mut read = ReadSession::open(&fixture.state, &fixture.id, "main", &operation).unwrap();
    let mut session = Counted {
        inner: &mut *read.session,
        metadata: 0,
        counts: 0,
        reads: 0,
        largest: 0,
        offsets: 0,
    };
    let mut reader =
        TableReader::new(&mut session, true, "main", "", "items", None, &operation).unwrap();
    assert!(reader.streaming);
    let mut count = 0;
    while let Some(batch) = reader.next(&operation).unwrap() {
        assert!(batch.len() <= 500);
        count += batch.len();
    }
    assert_eq!(count, 200003);
    assert!(!reader.truncated());
    drop(reader);
    assert_eq!(session.offsets, 0, "keyset reads must not use OFFSET");
    assert_eq!(
        (
            session.metadata,
            session.counts,
            session.reads,
            session.largest
        ),
        (1, 1, 401, 500)
    );
    drop(session);
    read.finish().unwrap();
}

#[test]
fn composite_key_scan_preserves_physical_column_order_and_every_row() {
    let fixture = Fixture::new("CREATE TABLE items(payload TEXT, b INTEGER NOT NULL, a TEXT NOT NULL, PRIMARY KEY(a,b)); WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1203) INSERT INTO items SELECT 'row-' || x, x % 7, printf('key-%04d',x / 7) FROM n;");
    let operation = OperationContext::detached();
    let mut read = ReadSession::open(&fixture.state, &fixture.id, "main", &operation).unwrap();
    let mut reader = TableReader::new(
        &mut *read.session,
        read.snapshot,
        "main",
        "",
        "items",
        None,
        &operation,
    )
    .unwrap();
    assert!(reader.streaming);
    assert_eq!(
        reader
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>(),
        ["payload", "b", "a"]
    );
    let mut payloads = std::collections::BTreeSet::new();
    while let Some(batch) = reader.next(&operation).unwrap() {
        for row in batch {
            let id = row[0]
                .as_str()
                .unwrap()
                .strip_prefix("row-")
                .unwrap()
                .parse::<u64>()
                .unwrap();
            assert_eq!(row[1], json!(id % 7));
            assert_eq!(row[2], json!(format!("key-{:04}", id / 7)));
            assert!(payloads.insert(id), "duplicate cursor row {id}");
        }
    }
    assert_eq!(payloads.len(), 1203);
    assert_eq!(payloads.first(), Some(&1));
    assert_eq!(payloads.last(), Some(&1203));
    assert!(!reader.truncated());
    drop(reader);
    read.finish().unwrap();
}

#[test]
fn sqlite_cursor_preserves_dynamic_storage_types_and_integer_index_seek() {
    for declaration in ["NUMERIC", "BLOB"] {
        let values = if declaration == "NUMERIC" {
            "SELECT printf('key-%04d',x), 'value' FROM n"
        } else {
            // A column with no affinity can contain both numeric and zero-padded
            // text keys. Neither may be converted using the declared BLOB type.
            "SELECT x, 'value' FROM n UNION ALL SELECT printf('%04d',x), 'value' FROM n"
        };
        let fixture = Fixture::new(&format!("CREATE TABLE items(id {declaration} PRIMARY KEY, payload TEXT); WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<603) INSERT INTO items {values};"));
        let operation = OperationContext::detached();
        let mut read = ReadSession::open(&fixture.state, &fixture.id, "main", &operation).unwrap();
        let mut reader = TableReader::new(
            &mut *read.session,
            read.snapshot,
            "main",
            "",
            "items",
            None,
            &operation,
        )
        .unwrap();
        assert!(reader.streaming);
        let mut keys = std::collections::BTreeSet::new();
        while let Some(batch) = reader.next(&operation).unwrap() {
            for row in batch {
                assert!(keys.insert(row[0].to_string()), "duplicate cursor value");
            }
        }
        if declaration == "NUMERIC" {
            assert_eq!(keys.len(), 603);
            assert!(keys.contains("\"key-0603\""));
        } else {
            assert_eq!(keys.len(), 1206);
            assert!(keys.contains("1"));
            assert!(keys.contains("\"0001\""));
            assert!(keys.contains("603"));
            assert!(keys.contains("\"0603\""));
        }
        assert!(!reader.truncated());
        drop(reader);
        read.finish().unwrap();
    }

    let fixture = Fixture::new("CREATE TABLE items(id INTEGER PRIMARY KEY); WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<603) INSERT INTO items SELECT x FROM n;");
    let operation = OperationContext::detached();
    let mut read = ReadSession::open(&fixture.state, &fixture.id, "main", &operation).unwrap();
    let mut reader = TableReader::new(
        &mut *read.session,
        read.snapshot,
        "main",
        "",
        "items",
        None,
        &operation,
    )
    .unwrap();
    assert_eq!(reader.next(&operation).unwrap().unwrap().len(), 500);
    let sql = reader.keyset_sql(500).unwrap();
    let plan = reader
        .session
        .execute("main", &format!("EXPLAIN QUERY PLAN {sql}"), None)
        .unwrap();
    let details = plan
        .rows
        .iter()
        .filter_map(|row| row[3].as_str())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        details.contains("SEARCH items USING INTEGER PRIMARY KEY"),
        "{details}"
    );
    assert!(!details.contains("TEMP B-TREE"), "{details}");
    drop(reader);
    read.finish().unwrap();
}

#[test]
fn sqlite_ambiguous_cursor_values_fall_back_before_the_first_batch() {
    for value in [
        "X'0102'",
        "9223372036854775807",
        "'has' || char(0) || 'nul'",
        "1e999",
    ] {
        let fixture = Fixture::new(&format!("CREATE TABLE items(id PRIMARY KEY, payload TEXT); WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1203) INSERT INTO items SELECT x,'value' FROM n; UPDATE items SET id={value} WHERE id=1203;"));
        let operation = OperationContext::detached();
        let mut read = ReadSession::open(&fixture.state, &fixture.id, "main", &operation).unwrap();
        let mut reader = TableReader::new(
            &mut *read.session,
            read.snapshot,
            "main",
            "",
            "items",
            Some(700),
            &operation,
        )
        .unwrap();
        assert!(
            !reader.streaming,
            "ambiguous value must not fail after page 500: {value}"
        );
        assert_eq!(reader.next(&operation).unwrap().unwrap().len(), 700);
        assert!(reader.next(&operation).unwrap().is_none());
        assert!(reader.truncated());
        drop(reader);
        read.finish().unwrap();
    }
}

struct ChangingDocumentSchema {
    reads: usize,
    cancel: Option<OperationContext>,
}

impl DbSession for ChangingDocumentSchema {
    fn engine(&self) -> Engine {
        Engine::Mongodb
    }
    fn is_document_store(&self) -> bool {
        true
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
    fn relation_kind(&mut self, _: &str, _: &str, _: &str) -> AppResult<Option<String>> {
        unreachable!()
    }
    fn execute(&mut self, _: &str, _: &str, _: Option<usize>) -> AppResult<QueryOutcome> {
        unreachable!()
    }
    fn list_columns(&mut self, _: &str, _: &str, _: &str) -> AppResult<Vec<ColumnMeta>> {
        // The sampled document need not have the same fields or order as the
        // bounded browse result. In particular its key index can disappear.
        let mut key = ColumnMeta::named("_id");
        key.primary_key = true;
        Ok(vec![ColumnMeta::named("sample_only"), key])
    }
    fn count_relation(&mut self, _: &str, _: &str, _: &str) -> AppResult<u64> {
        Ok(1)
    }
    fn browse_page(
        &mut self,
        _: &str,
        _: &str,
        _: &str,
        _: &[crate::models::FilterSpec],
        _: &[crate::models::SortSpec],
        limit: u32,
        offset: u32,
    ) -> AppResult<QueryOutcome> {
        assert_eq!((limit, offset), (CEILING, 0));
        self.reads += 1;
        if let Some(operation) = self.cancel.take() {
            operation.request_cancel()?;
            return Err(AppError::msg("driver interrupted"));
        }
        Ok(QueryOutcome {
            original_rows: None,
            columns: vec![ColumnMeta::named("_id")],
            rows: vec![vec![json!({"$oid": "507f1f77bcf86cd799439011"})]],
            rows_affected: 0,
            truncated: false,
        })
    }
}

#[test]
fn document_reader_uses_result_schema_without_reusing_sampled_key_indices() {
    let operation = OperationContext::detached();
    let mut session = ChangingDocumentSchema {
        reads: 0,
        cancel: None,
    };
    let mut reader =
        TableReader::new(&mut session, false, "test", "", "items", None, &operation).unwrap();
    assert!(!reader.streaming);
    let rows = reader.next(&operation).unwrap().unwrap();
    assert_eq!(reader.columns.len(), 1);
    assert_eq!(reader.columns[0].name, "_id");
    assert_eq!(rows[0][0], json!({"$oid": "507f1f77bcf86cd799439011"}));
    assert!(reader.next(&operation).unwrap().is_none());
    assert!(!reader.truncated());
    drop(reader);
    assert_eq!(session.reads, 1);
}

#[test]
fn interrupted_read_returns_cancelled_instead_of_a_driver_failure() {
    let operation = OperationContext::detached();
    let mut session = ChangingDocumentSchema {
        reads: 0,
        cancel: Some(operation.clone()),
    };
    let mut reader =
        TableReader::new(&mut session, false, "test", "", "items", None, &operation).unwrap();
    assert!(matches!(
        reader.next(&operation),
        Err(AppError::Cancelled { .. })
    ));
}

#[test]
fn export_streams_past_old_limit_and_keeps_all_supported_text_formats_valid() {
    let fixture = Fixture::new("CREATE TABLE items(id INTEGER PRIMARY KEY, payload TEXT); WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<200003) INSERT INTO items SELECT x, CASE WHEN x=1 THEN NULL ELSE 'quote,\"换行' || char(10) || x END FROM n;");
    let csv = fixture.request("csv");
    let csv_path = csv.path.clone();
    let result = export_file(&fixture.state, csv).unwrap();
    assert_eq!(result.row_count, 200003);
    assert!(!result.truncated);
    let mut parsed = csv::Reader::from_path(&csv_path).unwrap();
    assert_eq!(parsed.records().count(), 200003);
    // Exercise per-row serialization and escaping without repeating a 200k-row
    // scan for each format; the large CSV already proves the ceiling is gone.
    Connection::open(&fixture.database)
        .unwrap()
        .execute("DELETE FROM items WHERE id>1203", [])
        .unwrap();
    for format in ["json", "xml", "txt"] {
        let request = fixture.request(format);
        let path = request.path.clone();
        let result = export_file(&fixture.state, request).unwrap();
        assert_eq!(result.row_count, 1203);
        let text = fs::read_to_string(&path).unwrap();
        match format {
            "json" => {
                let rows: Vec<Value> = serde_json::from_str(&text).unwrap();
                assert_eq!(rows.len(), 1203);
                assert_eq!(rows[0]["payload"], Value::Null);
                assert_eq!(rows[1202]["id"], json!(1203));
            }
            "xml" => {
                assert_eq!(text.matches("<row>").count(), 1203);
                assert!(text.contains("&quot;换行"));
                assert!(text.ends_with("</rows>\n"));
            }
            _ => {
                let mut reader = csv::ReaderBuilder::new()
                    .delimiter(b'\t')
                    .from_reader(text.as_bytes());
                let records = reader.records().map(Result::unwrap).collect::<Vec<_>>();
                assert_eq!(records.len(), 1203);
                assert_eq!(&records[0][1], "\\N");
                assert!(records[1][1].contains('\n'));
            }
        }
    }
}

#[test]
fn snapshot_is_stable_across_pages_and_tables_and_accepts_sqlite_text_collations() {
    let fixture = Fixture::new("CREATE TABLE items(id TEXT COLLATE NOCASE NOT NULL, payload TEXT, PRIMARY KEY(id COLLATE BINARY)); CREATE TABLE other(id INTEGER PRIMARY KEY); INSERT INTO other VALUES(1); INSERT INTO items VALUES('key-0001','old'); WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1203) INSERT INTO items SELECT printf('Key-%04d',x), 'old' FROM n;");
    let op = OperationContext::detached();
    let mut read = ReadSession::open(&fixture.state, &fixture.id, "main", &op).unwrap();
    let mut reader = TableReader::new(
        &mut *read.session,
        read.snapshot,
        "main",
        "",
        "items",
        None,
        &op,
    )
    .unwrap();
    assert!(reader.streaming);
    let first = reader.next(&op).unwrap().unwrap();
    assert_eq!(first.len(), 500);
    let writer = Connection::open(&fixture.database).unwrap();
    writer
        .execute_batch("UPDATE items SET payload='new'; INSERT INTO other VALUES(2);")
        .unwrap();
    let mut loaded = first.len();
    while let Some(batch) = reader.next(&op).unwrap() {
        loaded += batch.len();
        assert!(batch.iter().all(|row| row[1] == json!("old")));
    }
    assert_eq!(loaded, 1204);
    drop(reader);
    let mut other = TableReader::new(
        &mut *read.session,
        read.snapshot,
        "main",
        "",
        "other",
        None,
        &op,
    )
    .unwrap();
    assert_eq!(other.total, 1);
    assert_eq!(other.next(&op).unwrap().unwrap().len(), 1);
    drop(other);
    read.finish().unwrap();
}

#[test]
fn no_key_and_nullable_keys_use_one_limited_read_and_cancellation_keeps_old_output() {
    let fixture = Fixture::new("CREATE TABLE items(id INTEGER PRIMARY KEY, payload TEXT); CREATE TABLE unkeyed(payload TEXT); CREATE TABLE nullable(id TEXT PRIMARY KEY, payload TEXT); INSERT INTO nullable VALUES(NULL,'a'),(NULL,'b'); WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1203) INSERT INTO items SELECT x, 'original' FROM n; INSERT INTO unkeyed SELECT payload FROM items;");
    let operation = OperationContext::detached();
    let mut read = ReadSession::open(&fixture.state, &fixture.id, "main", &operation).unwrap();
    for (table, snapshot) in [("unkeyed", true), ("nullable", true), ("items", false)] {
        let mut session = Counted {
            inner: &mut *read.session,
            metadata: 0,
            counts: 0,
            reads: 0,
            largest: 0,
            offsets: 0,
        };
        let mut reader = TableReader::new(
            &mut session,
            snapshot,
            "main",
            "",
            table,
            Some(700),
            &operation,
        )
        .unwrap();
        assert!(!reader.streaming);
        let expected = if table == "nullable" { 2 } else { 700 };
        assert_eq!(reader.next(&operation).unwrap().unwrap().len(), expected);
        assert!(reader.next(&operation).unwrap().is_none());
        assert_eq!(reader.truncated(), table != "nullable");
        drop(reader);
        assert_eq!(
            (
                session.metadata,
                session.counts,
                session.reads,
                session.largest
            ),
            (1, 1, 1, 700)
        );
    }
    read.finish().unwrap();
    let request = fixture.request("json");
    let path = request.path.clone();
    fs::write(&path, "previous output").unwrap();
    let result =
        export_streaming_file_with_hook(&fixture.state, request, &operation, &mut |loaded| {
            assert_eq!(loaded, 500);
            operation.request_cancel().unwrap();
        });
    assert!(matches!(result, Err(AppError::Cancelled { .. })));
    assert_eq!(fs::read_to_string(&path).unwrap(), "previous output");
    assert!(fs::read_dir(&fixture.directory).unwrap().all(|entry| !entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".tmp")));
}

#[test]
fn builtin_backup_streams_over_old_ceiling_and_late_bad_values_keep_previous_file() {
    let fixture = Fixture::new("CREATE TABLE items(id INTEGER PRIMARY KEY, payload INTEGER); WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<200003) INSERT INTO items SELECT x,x FROM n;");
    let request = fixture.backup();
    let result = backup_sql(&fixture.state, &request).unwrap();
    assert_eq!(result.row_count, 200003);
    let previous = fs::read(&request.path).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&previous)
            .matches("INSERT INTO")
            .count(),
        200003
    );
    Connection::open(&fixture.database).unwrap().execute_batch("DELETE FROM items WHERE id>1203; UPDATE items SET payload='not a number' WHERE id=1203;").unwrap();
    assert!(backup_sql(&fixture.state, &request).is_err());
    assert_eq!(fs::read(&request.path).unwrap(), previous);
}
