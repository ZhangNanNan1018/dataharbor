//! Disk keys use exact scalar equality; reject database comparison rules that
//! cannot be represented faithfully rather than guessing case/accent folding.
use super::{DbSession, Relation};
use crate::error::{AppError, AppResult};
use crate::models::Engine;
use crate::sqlutil::{quote_ident, sql_literal};
use crate::studio::sync::key_family;
use serde_json::Value;

fn text(value: Option<&Value>) -> &str {
    value.and_then(Value::as_str).unwrap_or("")
}
fn refused(column: &str) -> AppError {
    AppError::msg(format!("文本主键 {column} 的类型或排序规则不能确认具有精确、无填充的等价比较语义；已在写入前停止。请先对齐二进制排序规则和主键类型，或使用数据库原生迁移工具"))
}

pub(super) fn validate(session: &mut dyn DbSession, relation: &Relation<'_>) -> AppResult<()> {
    let keys = relation
        .columns
        .iter()
        .filter(|column| column.primary_key && key_family(&column.data_type) == "text")
        .map(|column| column.name.as_str())
        .collect::<Vec<_>>();
    if keys.is_empty() {
        return Ok(());
    }
    match session.engine() {
        Engine::Sqlite => sqlite(session, relation, &keys),
        Engine::Postgres => postgres(session, relation, &keys),
        Engine::Mysql => mysql(session, relation, &keys),
        // MariaDB variants do not consistently expose PAD_ATTRIBUTE. Do not
        // infer semantics from a collation name or silently change comparison.
        _ => Err(refused(keys[0])),
    }
}

fn sqlite(session: &mut dyn DbSession, relation: &Relation<'_>, keys: &[&str]) -> AppResult<()> {
    let schema = quote_ident(
        Engine::Sqlite,
        if relation.schema.is_empty() {
            "main"
        } else {
            relation.schema
        },
    )?;
    let table = quote_ident(Engine::Sqlite, relation.table)?;
    let indices = session.execute(
        relation.database,
        &format!("PRAGMA {schema}.index_list({table})"),
        None,
    )?;
    let primary = indices
        .rows
        .iter()
        .find(|row| text(row.get(3)) == "pk")
        .and_then(|row| row.get(1))
        .and_then(Value::as_str)
        .ok_or_else(|| refused(keys[0]))?;
    let index = quote_ident(Engine::Sqlite, primary)?;
    let details = session.execute(
        relation.database,
        &format!("PRAGMA {schema}.index_xinfo({index})"),
        None,
    )?;
    for key in keys {
        let row = details
            .rows
            .iter()
            .find(|row| text(row.get(2)) == *key)
            .ok_or_else(|| refused(key))?;
        if !text(row.get(4)).eq_ignore_ascii_case("BINARY") {
            return Err(refused(key));
        }
    }
    Ok(())
}

fn postgres(session: &mut dyn DbSession, relation: &Relation<'_>, keys: &[&str]) -> AppResult<()> {
    let schema = sql_literal(
        Engine::Postgres,
        &Value::String(if relation.schema.is_empty() {
            "public".into()
        } else {
            relation.schema.into()
        }),
    )?;
    let table = sql_literal(Engine::Postgres, &Value::String(relation.table.into()))?;
    let sql=format!("SELECT a.attname,t.typname,n.nspname,t.typtype,COALESCE(c.collisdeterministic,true)::text,t.oid::text FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_class r ON r.oid=a.attrelid JOIN pg_catalog.pg_namespace rn ON rn.oid=r.relnamespace JOIN pg_catalog.pg_type t ON t.oid=a.atttypid JOIN pg_catalog.pg_namespace n ON n.oid=t.typnamespace LEFT JOIN pg_catalog.pg_collation c ON c.oid=a.attcollation WHERE rn.nspname={schema} AND r.relname={table} AND a.attnum>0 AND NOT a.attisdropped");
    let result = session
        .execute(relation.database, &sql, None)
        .map_err(|_| refused(keys[0]))?;
    for key in keys {
        let row = result
            .rows
            .iter()
            .find(|row| text(row.first()) == *key)
            .ok_or_else(|| refused(key))?;
        let kind = text(row.get(1));
        if text(row.get(2)) != "pg_catalog"
            || text(row.get(3)) != "b"
            || matches!(kind, "bpchar" | "name" | "citext")
            || kind.starts_with('_')
            || !matches!(text(row.get(4)), "true" | "t" | "1")
            || text(row.get(5))
                .parse::<u32>()
                .map_or(true, |oid| oid >= 16384)
        {
            return Err(refused(key));
        }
    }
    Ok(())
}

fn mysql(session: &mut dyn DbSession, relation: &Relation<'_>, keys: &[&str]) -> AppResult<()> {
    let database = if relation.database.is_empty() {
        "DATABASE()".into()
    } else {
        sql_literal(Engine::Mysql, &Value::String(relation.database.into()))?
    };
    let table = sql_literal(Engine::Mysql, &Value::String(relation.table.into()))?;
    let sql=format!("SELECT c.COLUMN_NAME,c.COLLATION_NAME,s.PAD_ATTRIBUTE,c.DATA_TYPE FROM information_schema.COLUMNS c LEFT JOIN information_schema.COLLATIONS s ON s.COLLATION_NAME=c.COLLATION_NAME WHERE c.TABLE_SCHEMA={database} AND c.TABLE_NAME={table}");
    let result = session
        .execute(relation.database, &sql, None)
        .map_err(|_| refused(keys[0]))?;
    let prefixes=session.execute(relation.database,&format!("SELECT COLUMN_NAME,SUB_PART FROM information_schema.STATISTICS WHERE TABLE_SCHEMA={database} AND TABLE_NAME={table} AND INDEX_NAME='PRIMARY'"),None)?;
    for key in keys {
        let row = result
            .rows
            .iter()
            .find(|row| text(row.first()) == *key)
            .ok_or_else(|| refused(key))?;
        let kind = text(row.get(3));
        if !kind.contains("char") && !kind.contains("text") {
            continue;
        }
        let collation = text(row.get(1)).to_ascii_lowercase();
        let prefix = prefixes
            .rows
            .iter()
            .find(|row| text(row.first()) == *key)
            .ok_or_else(|| refused(key))?;
        if !collation.ends_with("_bin")
            || text(row.get(2)) != "NO PAD"
            || kind == "char"
            || prefix.get(1).is_none_or(|value| !value.is_null())
        {
            return Err(refused(key));
        }
    }
    Ok(())
}
