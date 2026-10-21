use serde_json::{Number, Value as JsonValue};

use crate::error::{AppError, AppResult};
use crate::models::Engine;

mod pagination;
mod scan;
pub use pagination::{is_read_query, paginate_query_sql, paginate_sql};

pub(crate) fn terminated_sql_prefix(sql: &str) -> Option<usize> {
    let scan = scan::scan(sql);
    scan.statements.iter().rev().nth(1).map(|(_, end)| end + 1)
}

pub(crate) fn sql_is_complete(sql: &str) -> bool {
    scan::scan(sql).complete
}

const JS_MAX: i64 = 9_007_199_254_740_991;

pub fn quote_ident(engine: Engine, name: &str) -> AppResult<String> {
    if name.is_empty() || name.contains('\0') {
        return Err(AppError::msg("非法标识符"));
    }
    match engine {
        Engine::Mysql | Engine::Mariadb => Ok(format!("`{}`", name.replace('`', "``"))),
        Engine::Mssql => Ok(format!("[{}]", name.replace(']', "]]"))),
        Engine::Postgres
        | Engine::Sqlite
        | Engine::Oracle
        | Engine::Mongodb
        | Engine::Redis
        | Engine::Snowflake
        | Engine::Dameng => Ok(format!("\"{}\"", name.replace('"', "\"\""))),
    }
}

pub fn sql_literal(engine: Engine, value: &JsonValue) -> AppResult<String> {
    match value {
        JsonValue::Null => Ok("NULL".to_string()),
        JsonValue::Bool(true) => Ok(match engine {
            Engine::Postgres => "TRUE".to_string(),
            _ => "1".to_string(),
        }),
        JsonValue::Bool(false) => Ok(match engine {
            Engine::Postgres => "FALSE".to_string(),
            _ => "0".to_string(),
        }),
        JsonValue::Number(number) => {
            if let Some(value) = number.as_i64() {
                Ok(value.to_string())
            } else if let Some(value) = number.as_u64() {
                Ok(value.to_string())
            } else if let Some(value) = number.as_f64() {
                if value.is_finite() {
                    Ok(value.to_string())
                } else {
                    Err(AppError::msg("非法数字"))
                }
            } else {
                Err(AppError::msg("非法数字"))
            }
        }
        JsonValue::String(text) => Ok(format!("'{}'", escape_string(engine, text))),
        JsonValue::Array(_) | JsonValue::Object(_) => {
            Ok(format!("'{}'", escape_string(engine, &value.to_string())))
        }
    }
}

fn escape_string(engine: Engine, text: &str) -> String {
    match engine {
        Engine::Mysql | Engine::Mariadb => text.replace('\\', "\\\\").replace('\'', "''"),
        _ => text.replace('\'', "''"),
    }
}

pub fn like_contains(engine: Engine, column_sql: &str, raw: &str) -> AppResult<String> {
    let mut pattern = String::from("%");
    for ch in raw.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            pattern.push('\\');
        }
        pattern.push(ch);
    }
    pattern.push('%');
    let literal = sql_literal(engine, &JsonValue::String(pattern))?;
    let escape = match engine {
        Engine::Mysql | Engine::Mariadb => "'\\\\'",
        _ => "'\\'",
    };
    Ok(format!("{column_sql} LIKE {literal} ESCAPE {escape}"))
}

pub fn qualified_name(
    engine: Engine,
    database: &str,
    schema: &str,
    table: &str,
) -> AppResult<String> {
    match engine {
        Engine::Sqlite => quote_ident(engine, table),
        Engine::Mysql | Engine::Mariadb => {
            if database.is_empty() {
                quote_ident(engine, table)
            } else {
                Ok(format!(
                    "{}.{}",
                    quote_ident(engine, database)?,
                    quote_ident(engine, table)?
                ))
            }
        }
        Engine::Dameng => {
            if schema.is_empty() {
                quote_ident(engine, table)
            } else {
                Ok(format!(
                    "{}.{}",
                    quote_ident(engine, schema)?,
                    quote_ident(engine, table)?
                ))
            }
        }
        Engine::Postgres | Engine::Oracle | Engine::Snowflake => {
            let schema = if schema.is_empty() {
                match engine {
                    Engine::Oracle => "USER",
                    Engine::Snowflake => "PUBLIC",
                    _ => "public",
                }
            } else {
                schema
            };
            Ok(format!(
                "{}.{}",
                quote_ident(engine, schema)?,
                quote_ident(engine, table)?
            ))
        }
        Engine::Mssql => {
            let schema = if schema.is_empty() { "dbo" } else { schema };
            Ok(format!(
                "{}.{}",
                quote_ident(engine, schema)?,
                quote_ident(engine, table)?
            ))
        }
        Engine::Mongodb | Engine::Redis => quote_ident(engine, table),
    }
}

pub fn json_i64(value: i64) -> JsonValue {
    if (-JS_MAX..=JS_MAX).contains(&value) {
        JsonValue::Number(value.into())
    } else {
        JsonValue::String(value.to_string())
    }
}

pub fn json_u64(value: u64) -> JsonValue {
    if value <= JS_MAX as u64 {
        JsonValue::Number(value.into())
    } else {
        JsonValue::String(value.to_string())
    }
}

pub fn json_f64(value: f64) -> JsonValue {
    Number::from_f64(value)
        .map(JsonValue::Number)
        .unwrap_or(JsonValue::Null)
}

pub fn json_blob(bytes: &[u8]) -> JsonValue {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    JsonValue::String(format!("base64:{}", STANDARD.encode(bytes)))
}

pub fn is_binary_type(data_type: &str) -> bool {
    let kind = data_type.to_ascii_lowercase();
    kind.contains("blob")
        || kind.contains("bytea")
        || kind.contains("binary")
        || kind.contains("raw")
        || kind == "image"
}

/// Decode a cell the grid stored as `base64:…` or as a Postgres/SQL Server hex literal.
pub fn binary_cell_bytes(value: &JsonValue) -> Option<Vec<u8>> {
    let JsonValue::String(text) = value else {
        return None;
    };
    if let Some(rest) = text.strip_prefix("base64:") {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        return STANDARD.decode(rest).ok();
    }
    let trimmed = text.trim();
    let hex = trimmed
        .strip_prefix("\\x")
        .or_else(|| trimmed.strip_prefix("\\X"))
        .or_else(|| trimmed.strip_prefix("0x"))
        .or_else(|| trimmed.strip_prefix("0X"))?;
    decode_hex(hex)
}

pub fn blob_literal(engine: Engine, bytes: &[u8]) -> AppResult<String> {
    let hex: String = bytes.iter().map(|byte| format!("{byte:02X}")).collect();
    Ok(match engine {
        Engine::Mysql | Engine::Mariadb | Engine::Sqlite => format!("X'{hex}'"),
        Engine::Postgres => format!("'\\x{hex}'::bytea"),
        Engine::Mssql => format!("0x{hex}"),
        Engine::Oracle | Engine::Dameng => format!("HEXTORAW('{hex}')"),
        Engine::Snowflake => format!("TO_BINARY('{hex}', 'HEX')"),
        Engine::Mongodb | Engine::Redis => {
            return Err(AppError::msg("文档库不用 SQL 字面量写二进制"));
        }
    })
}

pub fn value_sql(engine: Engine, data_type: &str, value: &JsonValue) -> AppResult<String> {
    if let JsonValue::String(text) = value {
        if is_numeric_type(data_type) {
            return exact_numeric_literal(text);
        }
        if data_type.to_ascii_lowercase().starts_with("bool") {
            match text.to_ascii_lowercase().as_str() {
                "1" | "t" | "true" | "yes" => return sql_literal(engine, &JsonValue::Bool(true)),
                "0" | "f" | "false" | "no" => return sql_literal(engine, &JsonValue::Bool(false)),
                _ => return Err(AppError::msg("布尔列需要 true/false 或 1/0")),
            }
        }
    }
    if is_binary_type(data_type) || (engine == Engine::Mssql && matches!(data_type.to_ascii_lowercase().as_str(), "rowversion" | "timestamp")) {
        if value.is_null() {
            return Ok("NULL".into());
        }
        if let JsonValue::String(text) = value {
            if text.is_empty() {
                return Ok("NULL".into());
            }
        }
        let bytes = binary_cell_bytes(value).ok_or_else(|| {
            AppError::msg("二进制列请用十六进制或图片替换，单元格需要是 base64: 或 \\x 十六进制")
        })?;
        return blob_literal(engine, &bytes);
    }
    sql_literal(engine, value)
}

pub(crate) fn is_numeric_type(data_type: &str) -> bool {
    let kind = data_type.to_ascii_lowercase();
    !kind.starts_with("interval") && !kind.contains('[') && ["int", "tinyint", "smallint", "mediumint", "bigint", "serial", "bigserial", "smallserial", "decimal", "numeric", "float", "double", "real", "money", "smallmoney", "number", "dec"]
        .iter().any(|prefix| kind.starts_with(prefix))
}

/// Validate decimal syntax without ever rounding through f64. The database
/// remains responsible for precision/range constraints of the target column.
pub(crate) fn exact_numeric_literal(text: &str) -> AppResult<String> {
    let text = text.trim();
    let bytes = text.as_bytes();
    let mut i = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let mut digits = 0;
    while bytes.get(i).is_some_and(u8::is_ascii_digit) { i += 1; digits += 1; }
    if bytes.get(i) == Some(&b'.') {
        i += 1;
        while bytes.get(i).is_some_and(u8::is_ascii_digit) { i += 1; digits += 1; }
    }
    if digits > 0 && matches!(bytes.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(bytes.get(i), Some(b'+' | b'-')) { i += 1; }
        let start = i;
        while bytes.get(i).is_some_and(u8::is_ascii_digit) { i += 1; }
        if start == i { return Err(AppError::msg("数值的指数不完整")); }
    }
    if digits == 0 || i != bytes.len() {
        return Err(AppError::msg("数值列包含无效数字；空字符串不能自动转换为 NULL，请使用 \\N"));
    }
    Ok(text.to_string())
}

pub fn reference_sql(
    engine: Engine,
    database: &str,
    schema: &str,
    table: &str,
    columns: &[&str],
    query: &str,
    limit: u32,
) -> AppResult<String> {
    let name = qualified_name(engine, database, schema, table)?;
    let body = if query.trim().is_empty() {
        format!("SELECT * FROM {name}")
    } else {
        let mut filters = Vec::new();
        let mut seen = Vec::new();
        for column in columns {
            let key = column.to_ascii_lowercase();
            if key.is_empty() || seen.iter().any(|item: &String| item == &key) {
                continue;
            }
            seen.push(key);
            let ident = quote_ident(engine, column)?;
            let cast = match engine {
                Engine::Mysql | Engine::Mariadb => format!("CAST({ident} AS CHAR)"),
                Engine::Mssql => format!("CAST({ident} AS NVARCHAR(400))"),
                Engine::Oracle | Engine::Dameng => format!("CAST({ident} AS VARCHAR(400))"),
                _ => format!("CAST({ident} AS TEXT)"),
            };
            filters.push(like_contains(engine, &cast, query.trim())?);
        }
        if filters.is_empty() {
            return Err(AppError::msg("没有可搜索的列"));
        }
        format!("SELECT * FROM {name} WHERE {}", filters.join(" OR "))
    };
    let limit = limit.clamp(1, 100);
    Ok(match engine {
        Engine::Mssql => {
            format!("{body} ORDER BY (SELECT NULL) OFFSET 0 ROWS FETCH NEXT {limit} ROWS ONLY")
        }
        Engine::Oracle | Engine::Dameng => {
            format!("SELECT * FROM ({body}) xb_ref WHERE ROWNUM <= {limit}")
        }
        _ => format!("{body} LIMIT {limit}"),
    })
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 || !text.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let pair = std::str::from_utf8(&bytes[index..index + 2]).ok()?;
        out.push(u8::from_str_radix(pair, 16).ok()?);
        index += 2;
    }
    Some(out)
}

pub fn json_count(value: &JsonValue) -> Option<u64> {
    match value {
        JsonValue::Number(number) => number
            .as_u64()
            .or_else(|| number.as_i64().map(|item| item as u64)),
        JsonValue::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

pub fn split_sql(input: &str) -> Vec<String> {
    scan::split(input)
}

pub fn strip_leading_comments(sql: &str) -> &str {
    let mut rest = sql.trim();
    loop {
        if let Some(stripped) = rest.strip_prefix("--") {
            if let Some(index) = stripped.find('\n') {
                rest = stripped[index + 1..].trim_start();
            } else {
                return "";
            }
        } else if let Some(stripped) = rest.strip_prefix("/*") {
            let bytes = stripped.as_bytes();
            let mut index = 0;
            let mut nesting = 1;
            while index < bytes.len() && nesting > 0 {
                if bytes.get(index..index + 2) == Some(b"/*") {
                    nesting += 1;
                    index += 2;
                } else if bytes.get(index..index + 2) == Some(b"*/") {
                    nesting -= 1;
                    index += 2;
                } else {
                    index += 1;
                }
            }
            if nesting > 0 {
                return "";
            }
            rest = stripped[index..].trim_start();
        } else {
            return rest;
        }
    }
}

pub fn is_pageable(sql: &str) -> bool {
    let word = strip_leading_comments(sql)
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    matches!(word.as_str(), "SELECT" | "WITH" | "VALUES" | "TABLE")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_strings_comments_and_dollars() {
        let sql = "select 'a;b'; select 2; -- tail\nselect $$x;y$$;";
        let parts = split_sql(sql);
        assert_eq!(parts.len(), 3);
        assert!(parts[0].contains("'a;b'"));
        assert_eq!(parts[1], "select 2");
        assert!(parts[2].contains("$$x;y$$"));
    }

    #[test]
    fn quotes_identifiers_and_literals() {
        assert_eq!(quote_ident(Engine::Mysql, "a`b").unwrap(), "`a``b`");
        assert_eq!(quote_ident(Engine::Sqlite, "a\"b").unwrap(), "\"a\"\"b\"");
        let literal = sql_literal(Engine::Mysql, &JsonValue::String("a'\\b".into())).unwrap();
        assert_eq!(literal, "'a''\\\\b'");
        assert_eq!(
            sql_literal(Engine::Postgres, &JsonValue::Bool(true)).unwrap(),
            "TRUE"
        );
    }

    #[test]
    fn detects_pageable_after_comments() {
        assert!(is_pageable("/* c */\nselect 1"));
        assert!(!is_pageable("update t set a = 1"));
        assert!(!is_pageable("show databases"));
    }

    #[test]
    fn blob_literals_and_reference_search_are_quoted() {
        let bytes = binary_cell_bytes(&JsonValue::String("base64:AP9o".into())).unwrap();
        assert_eq!(bytes, vec![0x00, 0xff, b'h']);
        assert_eq!(blob_literal(Engine::Sqlite, &bytes).unwrap(), "X'00FF68'");
        assert_eq!(
            blob_literal(Engine::Postgres, &bytes).unwrap(),
            "'\\x00FF68'::bytea"
        );
        assert_eq!(blob_literal(Engine::Mssql, &bytes).unwrap(), "0x00FF68");
        let sql =
            reference_sql(Engine::Sqlite, "", "", "authors", &["name"], "O'Hara", 20).unwrap();
        assert!(sql.contains("\"authors\""));
        assert!(sql.contains("O''Hara"));
        assert!(!sql.contains("O'Hara"));
        assert!(sql.contains("LIMIT 20"));
        let oracle = reference_sql(Engine::Oracle, "", "HR", "EMP", &["ENAME"], "", 10).unwrap();
        assert!(oracle.contains("ROWNUM <= 10"));
    }
}
