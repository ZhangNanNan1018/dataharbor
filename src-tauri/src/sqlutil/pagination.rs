//! Database paging syntax lives here so grids, exports and studio tools agree.
use crate::models::Engine;

/// Page an application-generated SELECT with no existing row limit.
/// Callers that require stable pages must supply a unique ORDER BY.
pub fn paginate_sql(engine: Engine, select: &str, limit: u32, offset: u32) -> String {
    let body = select.trim().trim_end_matches(';');
    match engine {
        Engine::Mssql => {
            let order = if super::scan::scan(body)
                .words
                .iter()
                .any(|word| word.depth == 0 && word.text == "ORDER")
            {
                ""
            } else {
                " ORDER BY (SELECT NULL)"
            };
            format!("{body}\n{order} OFFSET {offset} ROWS FETCH NEXT {limit} ROWS ONLY")
        }
        // OFFSET/FETCH keeps the result columns intact (Oracle 12c and newer).
        Engine::Oracle => format!("{body}\nOFFSET {offset} ROWS FETCH NEXT {limit} ROWS ONLY"),
        _ => format!("{body}\nLIMIT {limit} OFFSET {offset}"),
    }
}

/// Only rewrite ordinary read queries. Explicit limits, locking, SELECT INTO,
/// and data-modifying CTEs execute once as written, without a count query.
pub fn paginate_query_sql(engine: Engine, sql: &str, limit: u32, offset: u32) -> Option<String> {
    query_parts(engine, sql)?;
    Some(paginate_sql(engine, sql, limit, offset))
}

/// Export accepts query syntax, including explicit row limits, but never DML.
/// A SELECT may still call a user-defined function with its own side effects.
pub fn is_read_query(sql: &str) -> bool {
    let scan = super::scan::scan(sql);
    if !scan.complete {
        return false;
    }
    let tokens = scan.words;
    let top_level = tokens.iter().filter(|word| word.depth == 0).collect::<Vec<_>>();
    let mut selects = 0;
    for (index, word) in top_level.iter().enumerate() {
        if word.text != "SELECT" { continue; }
        selects += 1;
        if selects > 1 {
            let mut previous = index.checked_sub(1);
            if previous.is_some_and(|at| matches!(top_level[at].text.as_str(), "ALL" | "DISTINCT")) {
                previous = previous.and_then(|at| at.checked_sub(1));
            }
            if !previous.is_some_and(|at| matches!(top_level[at].text.as_str(), "UNION" | "INTERSECT" | "EXCEPT")) {
                return false;
            }
        }
    }
    matches!(
        tokens.first().map(|word| word.text.as_str()),
        Some("SELECT" | "WITH")
    ) && tokens
        .iter()
        .any(|word| word.depth == 0 && word.text == "SELECT")
        && !tokens.iter().any(|word| {
            // MySQL string/numeric functions share these DML/DDL spellings.
            // A call has an opening parenthesis after the token; a write
            // statement requires a relation/INTO/TABLE at that position.
            if matches!(word.text.as_str(), "INSERT" | "REPLACE" | "TRUNCATE")
                && super::strip_leading_comments(&sql[word.end..]).starts_with('(')
            {
                return false;
            }
            matches!(
                word.text.as_str(),
                "INSERT" | "UPDATE" | "DELETE" | "MERGE" | "INTO" | "CALL" | "EXEC" | "EXECUTE"
                // Some SQL dialects accept adjacent statements without a
                // semicolon. A leading SELECT cannot authorize those batches.
                | "CREATE" | "ALTER" | "DROP" | "TRUNCATE" | "REPLACE" | "UPSERT"
                | "GRANT" | "REVOKE" | "DENY" | "BEGIN" | "COMMIT" | "ROLLBACK"
                | "SAVEPOINT" | "RELEASE" | "SET" | "USE" | "PRAGMA" | "ATTACH"
                | "DETACH" | "VACUUM" | "REINDEX" | "COPY" | "LOAD" | "UNLOAD"
                | "BACKUP" | "RESTORE" | "DBCC" | "DO" | "DECLARE" | "KILL"
                | "LOCK" | "UNLOCK" | "HANDLER"
            )
        })
}

fn query_parts(engine: Engine, sql: &str) -> Option<()> {
    if !engine.is_sql() {
        return None;
    }
    let tokens = super::scan::scan(sql).words;
    if !is_read_query(sql) {
        return None;
    }
    if tokens.iter().any(|word| {
        word.depth == 0
            && matches!(
                word.text.as_str(),
                "LIMIT" | "OFFSET" | "FETCH" | "TOP" | "FOR" | "OPTION"
            )
    }) {
        return None;
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_grid_uses_engine_dialect_without_extra_columns() {
        let body = "SELECT id, name FROM books ORDER BY id";
        let mssql = paginate_sql(Engine::Mssql, body, 20, 40);
        assert!(mssql.ends_with("OFFSET 40 ROWS FETCH NEXT 20 ROWS ONLY"));
        assert_eq!(mssql.matches("ORDER BY").count(), 1);
        let oracle = paginate_sql(Engine::Oracle, body, 20, 40);
        assert!(!oracle.contains(" AS "));
        assert!(!oracle.contains("ROWNUM"));
        assert!(paginate_sql(Engine::Postgres, body, 20, 40).ends_with("LIMIT 20 OFFSET 40"));
        assert!(paginate_sql(Engine::Mssql, "SELECT id FROM books", 20, 0)
            .contains("ORDER BY (SELECT NULL)"));
    }

    #[test]
    fn quoted_or_nested_order_does_not_hide_required_mssql_order() {
        let sql = "SELECT [ORDER], 'ORDER BY', (SELECT MAX(id) FROM b ORDER BY 1) FROM books -- ORDER BY id";
        let paged = paginate_sql(Engine::Mssql, sql, 10, 0);
        assert!(paged.contains("\n ORDER BY (SELECT NULL) OFFSET"));
        assert!(paginate_query_sql(
            Engine::Postgres,
            "SELECT $$DELETE LIMIT$$, $tag$UPDATE$tag$",
            10,
            0
        )
        .is_some());
    }

    #[test]
    fn cte_paging_preserves_query_order_and_prefix() {
        let sql = "WITH x AS (SELECT id FROM books) SELECT id FROM x ORDER BY id";
        assert!(paginate_query_sql(Engine::Mssql, sql, 10, 20)
            .unwrap()
            .contains("ORDER BY id\n OFFSET 20"));
    }

    #[test]
    fn limited_and_mutating_queries_are_never_rewritten_or_counted() {
        for sql in [
            "SELECT TOP 5 * FROM t",
            "SELECT * FROM t LIMIT 5",
            "SELECT * FROM t FOR UPDATE",
            "SELECT * INTO backup FROM t",
            "WITH gone AS (DELETE FROM t RETURNING *) SELECT * FROM gone",
            "WITH x AS (SELECT 1) UPDATE t SET id=2",
            "UPDATE t SET id=2 RETURNING *",
        ] {
            assert!(
                paginate_query_sql(Engine::Mssql, sql, 10, 0).is_none(),
                "{sql}"
            );
        }
        assert!(is_read_query("SELECT * FROM t LIMIT 5"));
        assert!(!is_read_query("DELETE FROM t"));
        assert!(!is_read_query(
            "WITH gone AS (DELETE FROM t RETURNING *) SELECT * FROM gone"
        ));
    }
}
