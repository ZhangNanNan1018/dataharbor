use super::*;
use crate::models::ColumnMeta;
use serde_json::json;

#[derive(Default)]
struct PlanSession {
    calls: Vec<String>,
    fail_query: bool,
    fail_on: bool,
    fail_off: bool,
}

impl DbSession for PlanSession {
    fn engine(&self) -> Engine { Engine::Mssql }
    fn version(&mut self) -> AppResult<String> { Ok(String::new()) }
    fn list_databases(&mut self) -> AppResult<Vec<String>> { Ok(vec![]) }
    fn list_schemas(&mut self, _: &str) -> AppResult<Vec<String>> { Ok(vec![]) }
    fn list_tables(&mut self, _: &str, _: &str) -> AppResult<Vec<String>> { Ok(vec![]) }
    fn list_views(&mut self, _: &str, _: &str) -> AppResult<Vec<String>> { Ok(vec![]) }
    fn list_columns(&mut self, _: &str, _: &str, _: &str) -> AppResult<Vec<ColumnMeta>> { Ok(vec![]) }
    fn relation_kind(&mut self, _: &str, _: &str, _: &str) -> AppResult<Option<String>> { Ok(None) }
    fn execute(&mut self, _: &str, sql: &str, _: Option<usize>) -> AppResult<QueryOutcome> {
        self.calls.push(sql.into());
        if (sql == "SET SHOWPLAN_ALL ON" && self.fail_on) || (sql == "SELECT 1" && self.fail_query) || (sql == "SET SHOWPLAN_ALL OFF" && self.fail_off) { return Err(AppError::msg("fixture failure")); }
        let mut outcome = QueryOutcome::empty();
        if sql == "SELECT 1" {
            outcome.columns = vec![ColumnMeta::named("StmtText")];
            outcome.rows = vec![vec![json!("SELECT 1 plan")]];
        }
        Ok(outcome)
    }
}

#[test]
fn showplan_reads_query_result_and_always_attempts_off_after_query_failure() {
    for fail_query in [false, true] {
        for fail_off in [false, true] {
            let mut session = PlanSession { fail_query, fail_off, ..Default::default() };
            let result = explain_on_session(&mut session, "db", "SELECT 1");
            assert_eq!(session.calls, ["SET SHOWPLAN_ALL ON", "SELECT 1", "SET SHOWPLAN_ALL OFF"]);
            if !fail_query && !fail_off { assert_eq!(result.unwrap().rows, vec![vec![json!("SELECT 1 plan")]]); }
            else { assert!(result.is_err()); }
        }
    }
}

#[test]
fn showplan_enable_failure_still_attempts_cleanup_in_the_retired_session() {
    let mut session = PlanSession { fail_on: true, ..Default::default() };
    assert!(explain_on_session(&mut session, "db", "SELECT 1").is_err());
    assert_eq!(session.calls, ["SET SHOWPLAN_ALL ON", "SET SHOWPLAN_ALL OFF"]);
}

#[test]
fn bi_rejects_writes_batches_and_write_ctes_before_connecting() {
    for sql in [
        "DELETE FROM items", "UPDATE items SET id=2", "INSERT INTO items VALUES (1)",
        "WITH changed AS (DELETE FROM items RETURNING *) SELECT * FROM changed",
        "WITH c AS (SELECT 1) UPDATE items SET id=2", "SELECT 1; SELECT 2",
        "SELECT * INTO backup FROM items", "SELECT 1 DROP TABLE items",
        "SELECT 1 BEGIN TRANSACTION", "SELECT 1 EXEC sp_delete", "SELECT 'unterminated",
        "SELECT 1 SELECT 2", "SELECT 1 WITH c AS (SELECT 2) SELECT * FROM c",
    ] { assert!(read_query_statement(sql).is_err(), "{sql}"); }
    for sql in [
        "SELECT 'DELETE; DROP TABLE items' AS text", "WITH c AS (SELECT 1) SELECT * FROM c",
        "/* DELETE */ SELECT [UPDATE], \"DROP\" FROM items", "SELECT 1; -- trailing comment",
        "SELECT REPLACE(name,'a','b'), TRUNCATE(price,2), INSERT(name,1,2,'x') FROM items",
        "SELECT REPLACE /* comment */ (name, 'a', 'b') FROM items",
        "SELECT SUM(price) OVER (PARTITION BY category ORDER BY id) FROM items",
        "SELECT 1 UNION SELECT 2 UNION ALL SELECT 3",
    ] { assert!(read_query_statement(sql).is_ok(), "{sql}"); }
}
