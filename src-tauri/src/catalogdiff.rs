use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::Serialize;

use crate::error::AppResult;
use crate::models::{ColumnMeta, Engine};
use crate::sqlutil::{quote_ident, sql_literal};
use serde_json::Value as JsonValue;

#[derive(Debug, Clone, PartialEq)]
pub struct IndexSnap {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FkSnap {
    pub name: String,
    pub columns: Vec<String>,
    pub ref_table: String,
    pub ref_columns: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ObjectSnap {
    pub columns: Vec<ColumnMeta>,
    pub indexes: Vec<IndexSnap>,
    pub foreign_keys: Vec<FkSnap>,
    pub view_sql: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct SyncStatement {
    pub id: String,
    pub kind: String,
    pub sql: String,
    pub warning: bool,
}

#[cfg(test)]
fn composite_key(parts: &[String]) -> String {
    serde_json::to_string(parts).expect("string arrays always serialize")
}

pub fn diff_objects(
    engine: Engine,
    qualified: &str,
    source: &ObjectSnap,
    target: &ObjectSnap,
    allow_destructive: bool,
) -> (Vec<SyncStatement>, Vec<String>) {
    let mut statements = Vec::new();
    let mut notes = Vec::new();
    let push = |statements: &mut Vec<SyncStatement>, kind: &str, sql: String, warning: bool| {
        let identity =
            serde_json::to_vec(&(kind, &sql)).expect("statement identity always serializes");
        statements.push(SyncStatement {
            id: URL_SAFE_NO_PAD.encode(identity),
            kind: kind.into(),
            sql,
            warning,
        });
    };

    if let (Some(source_sql), Some(target_sql)) = (&source.view_sql, &target.view_sql) {
        if source_sql.trim() != target_sql.trim() {
            push(
                &mut statements,
                "view",
                format!("DROP VIEW {qualified};\nCREATE VIEW {qualified} AS\n{source_sql};"),
                true,
            );
        }
        return (statements, notes);
    }

    for column in &source.columns {
        if target
            .columns
            .iter()
            .any(|item| item.name.eq_ignore_ascii_case(&column.name))
        {
            if let Some(existing) = target
                .columns
                .iter()
                .find(|item| item.name.eq_ignore_ascii_case(&column.name))
            {
                if !existing.data_type.is_empty()
                    && !column.data_type.is_empty()
                    && !existing.data_type.eq_ignore_ascii_case(&column.data_type)
                {
                    let quoted =
                        quote_ident(engine, &column.name).unwrap_or_else(|_| column.name.clone());
                    let sql = match engine {
                        Engine::Mysql | Engine::Mariadb => {
                            format!(
                                "ALTER TABLE {qualified} MODIFY COLUMN {quoted} {};",
                                column.data_type
                            )
                        }
                        Engine::Postgres => format!(
                            "ALTER TABLE {qualified} ALTER COLUMN {quoted} TYPE {};",
                            column.data_type
                        ),
                        Engine::Mssql => format!(
                            "ALTER TABLE {qualified} ALTER COLUMN {quoted} {};",
                            column.data_type
                        ),
                        Engine::Sqlite => {
                            format!("-- SQLite 不能直接修改 {} 的类型，需要重建表", column.name)
                        }
                        _ => format!(
                            "-- 请手工调整列 {} 的类型为 {}",
                            column.name, column.data_type
                        ),
                    };
                    // Even syntactically valid ALTER TYPE can narrow existing data.
                    push(&mut statements, "modify", sql, true);
                    notes.push(format!(
                        "列 {} 类型 {} → {}",
                        column.name, existing.data_type, column.data_type
                    ));
                }
            }
            continue;
        }
        let quoted = quote_ident(engine, &column.name).unwrap_or_else(|_| column.name.clone());
        let data_type = if column.data_type.is_empty() {
            "TEXT".into()
        } else {
            column.data_type.clone()
        };
        let null_sql = if column.nullable { "NULL" } else { "NOT NULL" };
        let sql = match engine {
            Engine::Mssql => {
                format!("ALTER TABLE {qualified} ADD {quoted} {data_type} {null_sql};")
            }
            _ => format!("ALTER TABLE {qualified} ADD COLUMN {quoted} {data_type} {null_sql};"),
        };
        push(&mut statements, "add-column", sql, false);
    }

    for column in &target.columns {
        if source
            .columns
            .iter()
            .any(|item| item.name.eq_ignore_ascii_case(&column.name))
        {
            continue;
        }
        let quoted = quote_ident(engine, &column.name).unwrap_or_else(|_| column.name.clone());
        if allow_destructive {
            let sql = match engine {
                Engine::Mssql => format!("ALTER TABLE {qualified} DROP COLUMN {quoted};"),
                _ => format!("ALTER TABLE {qualified} DROP COLUMN {quoted};"),
            };
            push(&mut statements, "drop-column", sql, true);
        } else {
            notes.push(format!(
                "目标多出列 {}。勾选「包含删除」后才会生成 DROP。",
                column.name
            ));
        }
    }

    for index in &source.indexes {
        if target
            .indexes
            .iter()
            .any(|item| item.name.eq_ignore_ascii_case(&index.name))
        {
            continue;
        }
        let cols = index
            .columns
            .iter()
            .map(|column| quote_ident(engine, column).unwrap_or_else(|_| column.clone()))
            .collect::<Vec<_>>()
            .join(", ");
        if cols.is_empty() {
            continue;
        }
        let unique = if index.unique { "UNIQUE " } else { "" };
        let name = quote_ident(engine, &index.name).unwrap_or_else(|_| index.name.clone());
        push(
            &mut statements,
            "add-index",
            format!("CREATE {unique}INDEX {name} ON {qualified} ({cols});"),
            false,
        );
    }
    for index in &target.indexes {
        if source
            .indexes
            .iter()
            .any(|item| item.name.eq_ignore_ascii_case(&index.name))
            || index.name.starts_with("sqlite_autoindex")
        {
            continue;
        }
        if allow_destructive {
            let name = quote_ident(engine, &index.name).unwrap_or_else(|_| index.name.clone());
            push(
                &mut statements,
                "drop-index",
                format!("DROP INDEX {name};"),
                true,
            );
        } else {
            notes.push(format!("目标多出索引 {}。", index.name));
        }
    }

    for fk in &source.foreign_keys {
        let same = target.foreign_keys.iter().any(|item| {
            item.ref_table.eq_ignore_ascii_case(&fk.ref_table)
                && item.columns == fk.columns
                && item.ref_columns == fk.ref_columns
        });
        if same {
            continue;
        }
        let cols = fk
            .columns
            .iter()
            .map(|column| quote_ident(engine, column).unwrap_or_else(|_| column.clone()))
            .collect::<Vec<_>>()
            .join(", ");
        let refs = fk
            .ref_columns
            .iter()
            .map(|column| quote_ident(engine, column).unwrap_or_else(|_| column.clone()))
            .collect::<Vec<_>>()
            .join(", ");
        let ref_table = quote_ident(engine, &fk.ref_table).unwrap_or_else(|_| fk.ref_table.clone());
        let name = quote_ident(engine, &fk.name).unwrap_or_else(|_| fk.name.clone());
        let sql = match engine {
            Engine::Sqlite => format!("-- SQLite 外键 {name} 需要重建表才能添加"),
            _ => format!("ALTER TABLE {qualified} ADD CONSTRAINT {name} FOREIGN KEY ({cols}) REFERENCES {ref_table} ({refs});"),
        };
        push(&mut statements, "add-fk", sql, engine == Engine::Sqlite);
    }

    if statements.is_empty() {
        notes.push("结构一致，没有要生成的语句".into());
    }
    (statements, notes)
}

pub fn map_cell_for_engine(engine: Engine, value: &JsonValue) -> AppResult<String> {
    sql_literal(engine, value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, data_type: &str, pk: bool) -> ColumnMeta {
        ColumnMeta {
            name: name.into(),
            data_type: data_type.into(),
            nullable: !pk,
            primary_key: pk,
            auto_increment: false,
            default_value: None,
        }
    }

    #[test]
    fn composite_keys_do_not_collide() {
        assert_ne!(
            composite_key(&["a|b".into(), "c".into()]),
            composite_key(&["a".into(), "b|c".into()])
        );
        assert_ne!(
            composite_key(&["a\u{1f}b".into(), "c".into()]),
            composite_key(&["a".into(), "b\u{1f}c".into()])
        );
    }

    #[test]
    fn diff_adds_drops_and_modifies() {
        let source = ObjectSnap {
            columns: vec![
                col("id", "INTEGER", true),
                col("title", "TEXT", false),
                col("price", "REAL", false),
            ],
            indexes: vec![IndexSnap {
                name: "idx_title".into(),
                columns: vec!["title".into()],
                unique: false,
            }],
            foreign_keys: vec![FkSnap {
                name: "fk_author".into(),
                columns: vec!["author_id".into()],
                ref_table: "authors".into(),
                ref_columns: vec!["id".into()],
            }],
            view_sql: None,
        };
        let mut source = source;
        source.columns.insert(1, col("author_id", "INTEGER", false));
        let target = ObjectSnap {
            columns: vec![
                col("id", "INTEGER", true),
                col("title", "VARCHAR(20)", false),
                col("legacy", "TEXT", false),
            ],
            indexes: vec![],
            foreign_keys: vec![],
            view_sql: None,
        };
        let (statements, _) = diff_objects(
            Engine::Postgres,
            "\"public\".\"books\"",
            &source,
            &target,
            true,
        );
        let sql = statements
            .iter()
            .map(|item| item.sql.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(sql.contains("ADD COLUMN"));
        assert!(sql.contains("author_id"));
        assert!(sql.contains("DROP COLUMN"));
        assert!(sql.contains("legacy"));
        assert!(sql.contains("ALTER COLUMN"));
        assert!(sql.contains("CREATE INDEX"));
        assert!(sql.contains("FOREIGN KEY"));
        assert!(
            statements
                .iter()
                .find(|item| item.kind == "modify")
                .unwrap()
                .warning
        );
    }

    #[test]
    fn destructive_sql_is_omitted_until_requested() {
        let source = ObjectSnap {
            columns: vec![col("id", "INTEGER", true)],
            ..ObjectSnap::default()
        };
        let target = ObjectSnap {
            columns: vec![col("id", "INTEGER", true), col("extra", "TEXT", false)],
            ..ObjectSnap::default()
        };
        let (statements, notes) =
            diff_objects(Engine::Sqlite, "\"books\"", &source, &target, false);
        assert!(statements.iter().all(|item| item.kind != "drop-column"));
        assert!(notes.iter().any(|note| note.contains("extra")));
    }

    #[test]
    fn selection_identity_is_stable_across_order_changes_and_bound_to_sql() {
        let target = ObjectSnap::default();
        let mut source = ObjectSnap {
            columns: vec![col("a", "TEXT", false)],
            ..ObjectSnap::default()
        };
        let (before, _) = diff_objects(Engine::Sqlite, "\"items\"", &source, &target, false);
        source.columns.insert(0, col("new_first", "INTEGER", false));
        let (reordered, _) = diff_objects(Engine::Sqlite, "\"items\"", &source, &target, false);
        assert_eq!(before[0].id, reordered[1].id);
        assert_ne!(before[0].id, reordered[0].id);
        let (other_table, _) = diff_objects(Engine::Sqlite, "\"other\"", &source, &target, false);
        assert_ne!(reordered[1].id, other_table[1].id);
    }
}
