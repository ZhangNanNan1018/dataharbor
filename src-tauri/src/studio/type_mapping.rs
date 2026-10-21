//! Conservative cross-engine mappings. Unknown or lossy mappings require manual DDL.
use crate::error::{AppError, AppResult};
use crate::models::Engine;

pub(super) fn map_type(source_engine: Engine, target: Engine, source: &str) -> AppResult<String> {
    if source_engine == target {
        return Ok(source.into());
    }
    if !matches!(
        target,
        Engine::Mysql | Engine::Mariadb | Engine::Postgres | Engine::Mssql | Engine::Sqlite
    ) {
        return unsupported(source, target);
    }
    let kind = source
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if kind.contains('[') || kind.contains(']') || kind.contains("zerofill") {
        return unsupported(source, target);
    }
    let (base, args, suffix) = parse_type(&kind)
        .ok_or_else(|| AppError::msg(format!("无法可靠解析类型 {source}，请手工选择目标类型")))?;
    let unsigned = suffix == "unsigned";
    if !suffix.is_empty()
        && !unsigned
        && !matches!(suffix.as_str(), "with time zone" | "without time zone")
    {
        return unsupported(source, target);
    }
    let sql = match base.as_str() {
        "tinyint" | "smallint" | "int2" | "mediumint" | "int" | "integer" | "int4" | "bigint"
        | "int8" => {
            if args.len() > 1 {
                return unsupported(source, target);
            }
            let bits = match base.as_str() {
                "tinyint" => 8,
                "smallint" | "int2" => 16,
                "mediumint" => 24,
                "bigint" | "int8" => 64,
                _ => 32,
            };
            // SQLite's INTEGER affinity can also contain reals/text. A declared
            // type alone cannot establish a safe narrower target representation.
            if source_engine == Engine::Sqlite {
                return unsupported(source, target);
            }
            if target == Engine::Sqlite {
                if unsigned && bits == 64 {
                    return unsupported(source, target);
                }
                "INTEGER".into()
            } else if unsigned && matches!(target, Engine::Mysql | Engine::Mariadb) {
                format!(
                    "{} UNSIGNED",
                    if bits == 64 {
                        "BIGINT"
                    } else if bits > 16 {
                        "INT"
                    } else {
                        "SMALLINT"
                    }
                )
            } else if unsigned && bits == 64 {
                "DECIMAL(20,0)".into()
            } else {
                let bits = bits + if unsigned { 1 } else { 0 };
                if bits <= 16 {
                    "SMALLINT"
                } else if bits <= 32 {
                    "INTEGER"
                } else {
                    "BIGINT"
                }
                .into()
            }
        }
        "numeric" | "decimal" | "dec" | "number" => {
            if unsigned || target == Engine::Sqlite {
                return unsupported(source, target);
            }
            if args.is_empty() {
                if target == Engine::Postgres && source_engine == Engine::Postgres {
                    "NUMERIC".into()
                } else {
                    return unsupported(source, target);
                }
            } else {
                if args.len() > 2 {
                    return unsupported(source, target);
                }
                let precision = args[0];
                let scale = *args.get(1).unwrap_or(&0);
                let (max_precision, max_scale) = match target {
                    Engine::Postgres => (1000, 1000),
                    Engine::Mysql => (65, 30),
                    Engine::Mariadb => (65, 38),
                    _ => (38, 38),
                };
                if precision == 0
                    || precision > max_precision
                    || scale > precision
                    || scale > max_scale
                {
                    return unsupported(source, target);
                }
                format!("DECIMAL({precision},{scale})")
            }
        }
        "real" | "float4" | "float8" | "float" | "double" | "double precision" => {
            // Widen to a double without changing an exact decimal into a float.
            if args.len() > 1 || args.first().is_some_and(|bits| *bits > 53) {
                return unsupported(source, target);
            }
            match target {
                Engine::Postgres => "DOUBLE PRECISION",
                Engine::Mssql => "FLOAT(53)",
                Engine::Sqlite => "REAL",
                _ => "DOUBLE",
            }
            .into()
        }
        "boolean" | "bool" if args.is_empty() => match target {
            Engine::Mssql => "BIT",
            Engine::Sqlite => "INTEGER",
            _ => "BOOLEAN",
        }
        .into(),
        "bit" if source_engine == Engine::Mssql && args.is_empty() => match target {
            Engine::Sqlite => "INTEGER",
            _ => "BOOLEAN",
        }
        .into(),
        "varchar" | "character varying" | "nvarchar" | "char" | "character" | "nchar" => {
            if target == Engine::Sqlite {
                return unsupported(source, target);
            }
            if args.len() != 1 || args[0] == 0 {
                return unsupported(source, target);
            }
            let n = args[0];
            if (target == Engine::Mssql && n > 4000)
                || (matches!(target, Engine::Mysql | Engine::Mariadb) && n > 16383)
            {
                return unsupported(source, target);
            }
            let fixed = matches!(base.as_str(), "char" | "character" | "nchar");
            let name = match (target, fixed) {
                (Engine::Mssql, true) => "NCHAR",
                (Engine::Mssql, false) => "NVARCHAR",
                (_, true) => "CHAR",
                _ => "VARCHAR",
            };
            format!("{name}({n})")
        }
        "text" | "tinytext" | "mediumtext" | "longtext" | "ntext" if args.is_empty() => {
            if source_engine == Engine::Sqlite
                || (base == "longtext" && !matches!(target, Engine::Mysql | Engine::Mariadb))
            {
                return unsupported(source, target);
            }
            match target {
                Engine::Mssql => "NVARCHAR(MAX)",
                Engine::Mysql | Engine::Mariadb => "LONGTEXT",
                _ => "TEXT",
            }
            .into()
        }
        "bytea" | "blob" | "tinyblob" | "mediumblob" | "longblob" | "binary" | "varbinary" => {
            if source_engine == Engine::Sqlite
                || (base == "longblob" && !matches!(target, Engine::Mysql | Engine::Mariadb))
            {
                return unsupported(source, target);
            }
            if args.len() > 1 {
                return unsupported(source, target);
            }
            match target {
                Engine::Postgres => "BYTEA".into(),
                Engine::Mssql => "VARBINARY(MAX)".into(),
                Engine::Sqlite => "BLOB".into(),
                _ => {
                    if args.len() == 1 && args[0] > 0 && args[0] <= 65535 {
                        format!("VARBINARY({})", args[0])
                    } else {
                        "LONGBLOB".into()
                    }
                }
            }
        }
        "uuid" if args.is_empty() => match target {
            Engine::Mssql => "UNIQUEIDENTIFIER".into(),
            _ => return unsupported(source, target),
        },
        "uniqueidentifier" if args.is_empty() => match target {
            Engine::Postgres => "UUID".into(),
            _ => return unsupported(source, target),
        },
        "date" if args.is_empty() => {
            // Source ranges differ (PostgreSQL BC dates and SQL Server year 1).
            if target == Engine::Postgres && matches!(source_engine, Engine::Mssql) {
                "DATE".into()
            } else {
                return unsupported(source, target);
            }
        }
        "datetime2" => {
            let precision = args.first().copied().unwrap_or(7);
            if target != Engine::Postgres || args.len() > 1 || precision > 6 {
                return unsupported(source, target);
            }
            format!("TIMESTAMP({precision}) WITHOUT TIME ZONE")
        }
        "timestamp"
        | "timestamp without time zone"
        | "timestamp with time zone"
        | "timestamptz"
        | "datetime"
        | "datetimeoffset"
        | "time"
        | "time without time zone"
        | "time with time zone" => return unsupported(source, target),
        _ => return unsupported(source, target),
    };
    Ok(sql)
}

fn unsupported(source: &str, target: Engine) -> AppResult<String> {
    Err(AppError::msg(format!(
        "类型 {source} 无法保证无损映射到 {}，已拒绝自动生成结构变更；请手工指定兼容类型",
        target.label()
    )))
}

fn parse_type(kind: &str) -> Option<(String, Vec<u32>, String)> {
    if let Some((base, rest)) = kind.split_once('(') {
        let (args, suffix) = rest.split_once(')')?;
        let args = args
            .split(',')
            .map(|value| value.trim().parse::<u32>().ok())
            .collect::<Option<Vec<_>>>()?;
        Some((base.trim().into(), args, suffix.trim().into()))
    } else {
        let (base, suffix) = kind
            .strip_suffix(" unsigned")
            .map(|base| (base, "unsigned"))
            .unwrap_or((kind, ""));
        Some((base.into(), Vec::new(), suffix.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_exact_numeric_width_and_precision() {
        assert_eq!(
            map_type(Engine::Mysql, Engine::Postgres, "bigint").unwrap(),
            "BIGINT"
        );
        assert_eq!(
            map_type(Engine::Mysql, Engine::Postgres, "bigint unsigned").unwrap(),
            "DECIMAL(20,0)"
        );
        assert_eq!(
            map_type(Engine::Postgres, Engine::Mysql, "numeric(28,12)").unwrap(),
            "DECIMAL(28,12)"
        );
        assert!(map_type(Engine::Postgres, Engine::Mssql, "numeric(50,12)").is_err());
        assert!(map_type(Engine::Postgres, Engine::Mysql, "numeric").is_err());
        assert!(map_type(Engine::Postgres, Engine::Sqlite, "numeric(28,12)").is_err());
    }
    #[test]
    fn preserves_length_and_binary_representation() {
        assert_eq!(
            map_type(Engine::Mysql, Engine::Postgres, "varchar(120)").unwrap(),
            "VARCHAR(120)"
        );
        assert_eq!(
            map_type(Engine::Postgres, Engine::Mssql, "character varying(120)").unwrap(),
            "NVARCHAR(120)"
        );
        assert_eq!(
            map_type(Engine::Mysql, Engine::Postgres, "blob").unwrap(),
            "BYTEA"
        );
        assert_eq!(
            map_type(Engine::Postgres, Engine::Mssql, "uuid").unwrap(),
            "UNIQUEIDENTIFIER"
        );
    }
    #[test]
    fn rejects_unknown_and_time_semantics_instead_of_silently_using_text() {
        for kind in [
            "timestamp(6) with time zone",
            "timestamp without time zone",
            "date",
            "jsonb",
            "integer[]",
            "numeric(-1)",
        ] {
            assert!(
                map_type(Engine::Postgres, Engine::Mysql, kind).is_err(),
                "{kind}"
            );
        }
        assert_eq!(
            map_type(Engine::Mssql, Engine::Postgres, "datetime2(6)").unwrap(),
            "TIMESTAMP(6) WITHOUT TIME ZONE"
        );
        assert!(map_type(Engine::Mssql, Engine::Postgres, "datetime2(7)").is_err());
    }
}
