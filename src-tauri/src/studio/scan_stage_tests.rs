use super::*;
use crate::models::QueryOutcome;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

fn id_column() -> ColumnMeta {
    let mut column = ColumnMeta::named("id");
    column.primary_key = true;
    column.data_type = "INTEGER".into();
    column
}

#[test]
fn canonical_keys_keep_boundaries_types_and_exact_decimal_equivalence() {
    let mut text = ColumnMeta::named("tenant");
    text.primary_key = true;
    text.data_type = "TEXT".into();
    let numeric = id_column();
    let columns = [text.clone(), numeric];
    let names = ["tenant".into(), "id".into()];
    assert_eq!(
        canonical_key(&columns, &[json!("a,b"), json!("1.2300")], &names).unwrap(),
        canonical_key(&columns, &[json!("a,b"), json!("123e-2")], &names).unwrap()
    );
    assert_eq!(canonical_number("-0.00000e99").unwrap(), "0");
    assert_ne!(
        canonical_number("9007199254740992").unwrap(),
        canonical_number("9007199254740993").unwrap()
    );
    let text_columns = [
        text.clone(),
        ColumnMeta {
            name: "other".into(),
            ..text
        },
    ];
    let keys = ["tenant".into(), "other".into()];
    assert_ne!(
        canonical_key(&text_columns, &[json!("a|b"), json!("c")], &keys).unwrap(),
        canonical_key(&text_columns, &[json!("a"), json!("b|c")], &keys).unwrap()
    );
    assert!(canonical_key(&columns, &[json!("tenant"), Value::Null], &names).is_err());
}

struct BrokenRead {
    truncated: bool,
    duplicate: bool,
    ended: Arc<AtomicBool>,
}
impl DbSession for BrokenRead {
    fn engine(&self) -> Engine {
        Engine::Sqlite
    }
    fn version(&mut self) -> AppResult<String> {
        Ok("fixture".into())
    }
    fn list_databases(&mut self) -> AppResult<Vec<String>> {
        Ok(vec!["main".into()])
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
        Ok(vec![id_column()])
    }
    fn relation_kind(&mut self, _: &str, _: &str, _: &str) -> AppResult<Option<String>> {
        Ok(Some("table".into()))
    }
    fn begin_read_snapshot(&mut self, _: &str) -> AppResult<()> {
        Ok(())
    }
    fn end_read_snapshot(&mut self, _: &str) -> AppResult<()> {
        self.ended.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn execute(&mut self, _: &str, sql: &str, max: Option<usize>) -> AppResult<QueryOutcome> {
        assert!(max.unwrap_or(0) <= BATCH as usize);
        let mut result = QueryOutcome::empty();
        if sql.contains("COUNT(*)") {
            result.rows = vec![vec![json!(3)]];
        } else {
            result.columns = vec![id_column()];
            result.rows = vec![
                vec![json!(1)],
                vec![json!(if self.duplicate { 1 } else { 2 })],
            ];
            result.truncated = self.truncated;
        }
        Ok(result)
    }
}

#[test]
fn incomplete_duplicate_and_truncated_scans_close_snapshot_and_remove_private_stage() {
    let dir = std::env::temp_dir().join(format!("dataharbor-stage-test-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    for (truncated, duplicate) in [(false, false), (true, false), (false, true)] {
        let ended = Arc::new(AtomicBool::new(false));
        let mut driver = BrokenRead {
            truncated,
            duplicate,
            ended: ended.clone(),
        };
        {
            let mut stage = Stage::new(&dir).unwrap();
            let columns = [id_column()];
            let keys = ["id".into()];
            let relation = Relation {
                database: "main",
                schema: "",
                table: "items",
                columns: &columns,
                canonical_keys: &keys,
            };
            assert!(stage
                .scan(
                    Side::Source,
                    &mut driver,
                    &relation,
                    &OperationContext::detached(),
                    "读取源表",
                    &mut |_| {}
                )
                .is_err());
            assert!(ended.load(Ordering::SeqCst));
            assert!(stage.max_batch <= BATCH as usize);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    fs::metadata(&stage._directory.0)
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o700
                );
                assert_eq!(
                    fs::metadata(stage._directory.0.join("stage.sqlite"))
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o600
                );
            }
        }
        assert_eq!(fs::read_dir(dir.join("operations")).unwrap().count(), 0);
    }
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn keyset_paging_uses_every_composite_component_without_offset() {
    let mut first = id_column();
    first.name = "tenant".into();
    let second = id_column();
    let columns = [first, second];
    let sql = keyset_sql(
        Engine::Postgres,
        "items",
        &columns,
        &[0, 1],
        Some(&[json!(12), json!(34)]),
        500,
    )
    .unwrap();
    assert!(sql.contains("(\"tenant\" > 12) OR (\"tenant\" = 12 AND \"id\" > 34)"));
    assert!(!sql.contains("OFFSET"));
}
