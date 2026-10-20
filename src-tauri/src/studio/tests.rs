//! Isolated SQLite workflows for the data-loss boundaries in Studio commands.
use std::fs;
use std::path::PathBuf;

use serde_json::{json, Value};
use uuid::Uuid;

use super::backup::{backup_sql, backup_with_ceiling};
use super::project::import_project;
use super::storage::{read_vec, write_vec};
use super::sync::{diff_structure, sync_rows, sync_with_ceiling, transfer_with_ceiling};
use super::table_data::{ensure_complete, load_all, load_all_with_ceiling};
use super::types::{BackupRequest, Job, PairRequest};
use crate::models::{Engine, ExecuteSqlRequest, SaveConnectionRequest};
use crate::query::execute_sql;
use crate::state::AppState;

struct Fixture {
    state: AppState,
    directory: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory =
            std::env::temp_dir().join(format!("dataharbor-studio-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let state = AppState::open_in(directory.clone(), true).unwrap();
        Self { state, directory }
    }

    fn database(&self, name: &str, sql: &str) -> String {
        let saved = self
            .state
            .save_connection(SaveConnectionRequest {
                name: name.into(),
                engine: Engine::Sqlite,
                file_path: self
                    .directory
                    .join(format!("{name}.sqlite"))
                    .display()
                    .to_string(),
                ..SaveConnectionRequest::default()
            })
            .unwrap();
        let id = saved.connection.id;
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
                page_size: 500,
                only_statement: None,
            },
        )
        .unwrap();
        assert!(
            result.statements.iter().all(|statement| statement.ok),
            "{result:?}"
        );
    }

    fn rows(&self, id: &str, table: &str) -> Vec<Vec<Value>> {
        load_all(&self.state, id, "main", "", table).unwrap().1
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
    serde_json::from_value(json!({
        "sourceId": source, "sourceDatabase": "main", "sourceTable": "items",
        "targetId": target, "targetDatabase": "main", "targetTable": "items",
        "deleteMissing": true
    }))
    .unwrap()
}

#[test]
fn table_read_keeps_primary_keys_and_total_across_pages() {
    let fixture = Fixture::new();
    let id = fixture.database(
        "paged",
        "CREATE TABLE items (payload TEXT, id INTEGER PRIMARY KEY);",
    );
    let rows = (1..=503)
        .rev()
        .map(|id| format!("('row-{id}', {id})"))
        .collect::<Vec<_>>()
        .join(",");
    fixture.execute(&id, &format!("INSERT INTO items VALUES {rows};"));
    let (columns, rows, total, truncated) =
        load_all(&fixture.state, &id, "main", "", "items").unwrap();
    assert_eq!(total, 503);
    assert_eq!(rows.len(), 503);
    assert!(!truncated);
    assert!(!columns[0].primary_key);
    assert!(columns[1].primary_key);
    assert_eq!(rows[0][1], json!(1));
    assert_eq!(rows[502][1], json!(503));
}

#[test]
fn exact_read_limit_is_complete_and_larger_table_is_rejected() {
    let fixture = Fixture::new();
    let id = fixture.database("bounded", "CREATE TABLE items (value TEXT, id INTEGER PRIMARY KEY); INSERT INTO items VALUES ('a', 1), ('b', 2), ('c', 3);");
    let (_, rows, total, truncated) =
        load_all_with_ceiling(&fixture.state, &id, "main", "", "items", 3).unwrap();
    assert_eq!(total, 3);
    assert_eq!(rows.len(), 3);
    assert!(!truncated);
    ensure_complete(total, rows.len(), truncated, "测试").unwrap();
    let (_, rows, total, truncated) =
        load_all_with_ceiling(&fixture.state, &id, "main", "", "items", 2).unwrap();
    assert_eq!(total, 3);
    assert_eq!(rows.len(), 2);
    assert!(truncated);
    assert!(ensure_complete(total, rows.len(), truncated, "测试").is_err());
}

#[test]
fn unkeyed_tables_are_read_completely_beyond_the_page_size() {
    let fixture = Fixture::new();
    let id = fixture.database("unkeyed_read", "CREATE TABLE items (payload TEXT);");
    let values = (1..=503)
        .map(|index| format!("('row-{index}')"))
        .collect::<Vec<_>>()
        .join(",");
    fixture.execute(&id, &format!("INSERT INTO items VALUES {values};"));
    let (columns, rows, total, truncated) =
        load_all(&fixture.state, &id, "main", "", "items").unwrap();
    assert_eq!(rows.len(), 503);
    assert_eq!(total, 503);
    assert!(!truncated);
    assert!(!columns[0].primary_key);
    let unique = rows
        .iter()
        .map(|row| row[0].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(unique.len(), 503);
}

#[test]
fn sync_handles_reordered_columns_and_collision_prone_composite_keys() {
    let fixture = Fixture::new();
    let source = fixture.database(
        "composite_source",
        "CREATE TABLE items (payload TEXT, a TEXT, b TEXT, PRIMARY KEY (a, b));",
    );
    fixture.execute(&source, &format!("INSERT INTO items VALUES ('same', 'a\u{1f}b', 'c'), ('same', 'a', 'b\u{1f}c'), ('updated', 'z', '1');"));
    let target = fixture.database("composite_target", "CREATE TABLE items (b TEXT, payload TEXT, a TEXT, PRIMARY KEY (a, b)); INSERT INTO items VALUES ('1', 'old', 'z'), ('gone', 'extra', 'remove');");
    let result = sync_rows(&fixture.state, &pair(&source, &target)).unwrap();
    assert_eq!((result.inserted, result.updated, result.deleted), (2, 1, 1));
    let rows = fixture.rows(&target, "items");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows.iter().filter(|row| row[1] == json!("same")).count(), 2);
    assert!(rows.iter().any(|row| row[1] == json!("updated")));
}

#[test]
fn sync_rejects_missing_or_incompatible_primary_keys_before_mutation() {
    let fixture = Fixture::new();
    let source = fixture.database(
        "unkeyed",
        "CREATE TABLE items (value TEXT, id INTEGER); INSERT INTO items VALUES ('source', 1);",
    );
    let target = fixture.database("keyed", "CREATE TABLE items (value TEXT, id INTEGER PRIMARY KEY); INSERT INTO items VALUES ('keep', 2);");
    assert!(sync_rows(&fixture.state, &pair(&source, &target)).is_err());
    assert_eq!(
        fixture.rows(&target, "items"),
        vec![vec![json!("keep"), json!(2)]]
    );
    let incompatible = fixture.database("other_key", "CREATE TABLE items (value TEXT PRIMARY KEY, id INTEGER); INSERT INTO items VALUES ('keep-too', 3);");
    assert!(sync_rows(&fixture.state, &pair(&target, &incompatible)).is_err());
    assert_eq!(
        fixture.rows(&incompatible, "items"),
        vec![vec![json!("keep-too"), json!(3)]]
    );
}

#[test]
fn truncated_source_or_target_cannot_start_transfer_sync_or_delete() {
    let fixture = Fixture::new();
    let large = fixture.database("large", "CREATE TABLE items (id INTEGER PRIMARY KEY, value TEXT); INSERT INTO items VALUES (1, 'a'), (2, 'b'), (3, 'c');");
    let small = fixture.database("small", "CREATE TABLE items (id INTEGER PRIMARY KEY, value TEXT); INSERT INTO items VALUES (9, 'keep');");
    let before_large = fixture.rows(&large, "items");
    let before_small = fixture.rows(&small, "items");
    assert!(transfer_with_ceiling(&fixture.state, &pair(&large, &small), 2).is_err());
    assert!(sync_with_ceiling(&fixture.state, &pair(&large, &small), 2).is_err());
    assert!(sync_with_ceiling(&fixture.state, &pair(&small, &large), 2).is_err());
    assert_eq!(fixture.rows(&large, "items"), before_large);
    assert_eq!(fixture.rows(&small, "items"), before_small);
}

#[test]
fn transfer_and_sync_reject_the_same_table_even_without_a_primary_key() {
    let fixture = Fixture::new();
    let id = fixture.database(
        "self_transfer",
        "CREATE TABLE items (value TEXT); INSERT INTO items VALUES ('keep');",
    );
    let request = pair(&id, &id);
    assert!(transfer_with_ceiling(&fixture.state, &request, 2).is_err());
    assert!(sync_with_ceiling(&fixture.state, &request, 2).is_err());
    assert_eq!(fixture.rows(&id, "items"), vec![vec![json!("keep")]]);
}

#[test]
fn structure_sync_requires_a_current_nonempty_selection() {
    let fixture = Fixture::new();
    let source = fixture.database(
        "structure_source",
        "CREATE TABLE items (id INTEGER PRIMARY KEY, value TEXT);",
    );
    let target = fixture.database(
        "structure_target",
        "CREATE TABLE items (id INTEGER PRIMARY KEY);",
    );
    let mut request = pair(&source, &target);
    let preview = diff_structure(&fixture.state, &request).unwrap();
    let selected = preview
        .statements
        .iter()
        .find(|item| item.kind == "add-column")
        .unwrap()
        .id
        .clone();
    request.apply = true;
    assert!(diff_structure(&fixture.state, &request).is_err());
    request.selected_ids = vec!["stale-preview-id".into()];
    assert!(diff_structure(&fixture.state, &request).is_err());
    let columns = fixture
        .state
        .with_session(&target, |session| session.list_columns("main", "", "items"))
        .unwrap();
    assert_eq!(columns.len(), 1);
    request.selected_ids = vec![selected];
    assert!(diff_structure(&fixture.state, &request).unwrap().applied);
    assert!(diff_structure(&fixture.state, &request).is_err());
}

#[test]
fn sqlite_script_includes_more_than_forty_tables_and_restores_rows() {
    let fixture = Fixture::new();
    let sql = (0..41).map(|index| format!("CREATE TABLE t{index} (id INTEGER PRIMARY KEY, bytes BLOB); INSERT INTO t{index} VALUES ({index}, X'00FF');")).collect::<String>();
    let id = fixture.database("many_tables", &sql);
    let output = fixture.directory.join("tables.sql");
    let result = backup_sql(
        &fixture.state,
        &BackupRequest {
            connection_id: id,
            database: "main".into(),
            schema: String::new(),
            path: output.display().to_string(),
            mode: "builtin".into(),
        },
    )
    .unwrap();
    assert_eq!(result.row_count, 41);
    assert!(!result.truncated);
    assert_eq!(result.kind, "sqlite-script");
    assert!(!PathBuf::from(format!("{}.sqlite-copy", output.display())).exists());
    let restored = rusqlite::Connection::open_in_memory().unwrap();
    restored
        .execute_batch(&fs::read_to_string(output).unwrap())
        .unwrap();
    let count: i64 = restored
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 41);
    let value: i64 = restored
        .query_row("SELECT id FROM t40", [], |row| row.get(0))
        .unwrap();
    assert_eq!(value, 40);
    let (kind, hex): (String, String) = restored
        .query_row("SELECT typeof(bytes), hex(bytes) FROM t40", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!((kind.as_str(), hex.as_str()), ("blob", "00FF"));
}

#[test]
fn incomplete_backup_preserves_existing_output_file() {
    let fixture = Fixture::new();
    let id = fixture.database(
        "backup_limit",
        "CREATE TABLE items (id INTEGER PRIMARY KEY); INSERT INTO items VALUES (1), (2), (3);",
    );
    let output = fixture.directory.join("preserved.sql");
    fs::write(&output, "existing artifact").unwrap();
    let request = BackupRequest {
        connection_id: id,
        database: "main".into(),
        schema: String::new(),
        path: output.display().to_string(),
        mode: "builtin".into(),
    };
    assert!(backup_with_ceiling(&fixture.state, &request, 2).is_err());
    assert_eq!(fs::read_to_string(output).unwrap(), "existing artifact");
}

#[test]
fn imported_jobs_stay_disabled_and_cannot_replace_existing_jobs() {
    let fixture = Fixture::new();
    let id = fixture.database(
        "jobs",
        "CREATE TABLE items (id INTEGER PRIMARY KEY); INSERT INTO items VALUES (1);",
    );
    let local: Job = serde_json::from_value(json!({ "id": "local", "name": "已有任务", "kind": "query", "connectionId": id, "database": "main", "sql": "SELECT 1", "enabled": true, "nextRunAt": u64::MAX })).unwrap();
    write_vec(&fixture.state, "jobs.json", &[local]).unwrap();
    let path = fixture.directory.join("import.json");
    fs::write(&path, serde_json::to_vec(&json!({
        "version": 1,
        "profiles": [{"id": "new-imported-profile", "name": "导入连接", "engine": "sqlite", "filePath": fixture.directory.join("imported.sqlite").display().to_string()}],
        "snippets": [], "dashboards": [], "models": [], "favorites": [],
        "jobs": [
            { "id": "local", "name": "覆盖任务", "kind": "query", "connectionId": id, "sql": "DELETE FROM items", "enabled": true },
            { "id": "imported", "name": "导入任务", "kind": "query", "connectionId": id, "database": "main", "sql": "DELETE FROM items", "enabled": true }
        ]
    })).unwrap()).unwrap();
    let result = import_project(&fixture.state, path.to_str().unwrap()).unwrap();
    assert!(fixture
        .state
        .profiles_snapshot()
        .iter()
        .any(|profile| profile.id == "new-imported-profile"));
    assert!(result.contains("均已禁用"));
    assert!(result.contains("跳过同 ID 任务 1 个"));
    let jobs: Vec<Job> = read_vec(&fixture.state, "jobs.json");
    let existing = jobs.iter().find(|job| job.id == "local").unwrap();
    assert_eq!(existing.sql, "SELECT 1");
    assert!(existing.enabled);
    assert!(
        !jobs
            .iter()
            .find(|job| job.id == "imported")
            .unwrap()
            .enabled
    );
    assert!(super::automation::run_due_jobs(&fixture.state)
        .unwrap()
        .is_empty());
    assert_eq!(fixture.rows(&id, "items"), vec![vec![json!(1)]]);
}
