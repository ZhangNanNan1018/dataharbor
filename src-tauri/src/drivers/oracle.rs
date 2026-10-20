use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::Value as JsonValue;

use super::DbSession;
use crate::error::{AppError, AppResult};
use crate::models::{ColumnMeta, Engine, QueryOutcome, SessionConfig};

pub struct OracleSession {
    user: String,
    password: String,
    connect: String,
}

pub fn connect(config: &SessionConfig) -> AppResult<OracleSession> {
    if config.use_ssl {
        return Err(AppError::msg("当前 Oracle SQL*Plus 连接仅实现 TCP，未实现 TCPS / 钱包配置，不能启用 SSL；程序不会退回明文连接。"));
    }
    if Command::new("sqlplus").arg("-v").output().is_err() {
        return Err(AppError::msg(
            "这台机器没有 SQL*Plus。Oracle 官方协议需要 Instant Client（libclntsh），为了让 Windows 和 macOS 的默认构建不必安装专有客户端，DataHarbor 通过本机的 sqlplus 执行。请安装 Oracle Instant Client 并把 sqlplus 放到 PATH。",
        ));
    }
    let port = if config.port == 0 { 1521 } else { config.port };
    let service = if config.database.trim().is_empty() {
        "ORCL"
    } else {
        config.database.trim()
    };
    let session = OracleSession {
        user: config.username.clone(),
        password: config.password.clone(),
        connect: format!("//{}:{}/{}", config.host.trim(), port, service),
    };
    let version = session.query("SELECT banner FROM v$version WHERE ROWNUM = 1")?;
    if version.rows.is_empty() && version.columns.is_empty() {
        return Err(AppError::msg(
            "SQL*Plus 没有返回版本，请检查账号、主机和服务名",
        ));
    }
    Ok(session)
}

impl OracleSession {
    fn query(&self, sql: &str) -> AppResult<QueryOutcome> {
        let script = sqlplus_script(&self.user, &self.password, &self.connect, sql);
        let mut child = Command::new("sqlplus")
            .args(["-S", "/nolog"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| AppError::msg(format!("无法启动 sqlplus: {err}")))?;
        if let Some(stdin) = child.stdin.as_mut() {
            stdin
                .write_all(script.as_bytes())
                .map_err(|err| AppError::msg(err.to_string()))?;
        }
        let output = child
            .wait_with_output()
            .map_err(|err| AppError::msg(err.to_string()))?;
        let text = String::from_utf8_lossy(&output.stdout);
        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            return Err(AppError::msg(format!("{text}{err}").trim().to_string()));
        }
        if text.to_ascii_lowercase().contains("ora-") || text.to_ascii_lowercase().contains("sp2-")
        {
            return Err(AppError::msg(
                text.lines()
                    .find(|line| {
                        line.to_ascii_lowercase().contains("ora-")
                            || line.to_ascii_lowercase().contains("sp2-")
                    })
                    .unwrap_or(text.trim())
                    .to_string(),
            ));
        }
        Ok(parse_sqlplus(&text))
    }
}

impl DbSession for OracleSession {
    fn engine(&self) -> Engine {
        Engine::Oracle
    }

    fn version(&mut self) -> AppResult<String> {
        let outcome = self.query("SELECT banner FROM v$version WHERE ROWNUM = 1")?;
        Ok(outcome
            .rows
            .first()
            .and_then(|row| row.first())
            .and_then(|value| value.as_str())
            .unwrap_or("Oracle")
            .to_string())
    }

    fn list_databases(&mut self) -> AppResult<Vec<String>> {
        Ok(vec![self
            .connect
            .split('/')
            .next_back()
            .unwrap_or("ORCL")
            .to_string()])
    }

    fn list_schemas(&mut self, _database: &str) -> AppResult<Vec<String>> {
        let outcome = self.query("SELECT username FROM all_users ORDER BY username")?;
        Ok(column_strings(&outcome))
    }

    fn list_tables(&mut self, _database: &str, schema: &str) -> AppResult<Vec<String>> {
        let owner = schema.replace('\'', "''");
        let outcome = self.query(&format!(
            "SELECT table_name FROM all_tables WHERE owner = '{owner}' ORDER BY table_name"
        ))?;
        Ok(column_strings(&outcome))
    }

    fn list_views(&mut self, _database: &str, schema: &str) -> AppResult<Vec<String>> {
        let owner = schema.replace('\'', "''");
        let outcome = self.query(&format!(
            "SELECT view_name FROM all_views WHERE owner = '{owner}' ORDER BY view_name"
        ))?;
        Ok(column_strings(&outcome))
    }

    fn list_columns(
        &mut self,
        _database: &str,
        schema: &str,
        object_name: &str,
    ) -> AppResult<Vec<ColumnMeta>> {
        let owner = schema.replace('\'', "''");
        let name = object_name.replace('\'', "''");
        let outcome = self.query(&format!(
            "SELECT c.column_name, c.data_type, c.nullable, CASE WHEN p.column_name IS NULL THEN 'N' ELSE 'Y' END FROM all_tab_columns c LEFT JOIN (SELECT acc.column_name, acc.table_name, acc.owner FROM all_cons_columns acc JOIN all_constraints ac ON ac.constraint_name = acc.constraint_name AND ac.owner = acc.owner WHERE ac.constraint_type = 'P') p ON p.owner = c.owner AND p.table_name = c.table_name AND p.column_name = c.column_name WHERE c.owner = '{owner}' AND c.table_name = '{name}' ORDER BY c.column_id"
        ))?;
        Ok(outcome
            .rows
            .into_iter()
            .map(|row| ColumnMeta {
                name: text_at(&row, 0),
                data_type: text_at(&row, 1),
                nullable: text_at(&row, 2).eq_ignore_ascii_case("Y"),
                primary_key: text_at(&row, 3).eq_ignore_ascii_case("Y"),
                auto_increment: false,
                default_value: None,
            })
            .filter(|column| !column.name.is_empty())
            .collect())
    }

    fn relation_kind(
        &mut self,
        _database: &str,
        schema: &str,
        object_name: &str,
    ) -> AppResult<Option<String>> {
        let owner = schema.replace('\'', "''");
        let name = object_name.replace('\'', "''");
        let views = self.query(&format!(
            "SELECT view_name FROM all_views WHERE owner = '{owner}' AND view_name = '{name}'"
        ))?;
        if !views.rows.is_empty() {
            return Ok(Some("view".into()));
        }
        let tables = self.query(&format!(
            "SELECT table_name FROM all_tables WHERE owner = '{owner}' AND table_name = '{name}'"
        ))?;
        if tables.rows.is_empty() {
            Ok(None)
        } else {
            Ok(Some("table".into()))
        }
    }

    fn execute(
        &mut self,
        _database: &str,
        sql: &str,
        max_rows: Option<usize>,
    ) -> AppResult<QueryOutcome> {
        let mut outcome = self.query(sql)?;
        if let Some(max) = max_rows {
            if outcome.rows.len() > max {
                outcome.rows.truncate(max);
                outcome.truncated = true;
            }
        }
        Ok(outcome)
    }

    fn execute_transaction(&mut self, _database: &str, statements: &[String]) -> AppResult<()> {
        if statements.is_empty() {
            return Ok(());
        }
        // Every statement, rollback handler and commit runs in this one process.
        let block = transaction_block(statements)?;
        self.query(&block)?;
        Ok(())
    }
}

fn sqlplus_script(user: &str, password: &str, connect: &str, sql: &str) -> String {
    let sql = sql.trim();
    let executable = if sql.ends_with('/') {
        sql.to_string()
    } else {
        format!("{};", sql.trim_end_matches(';'))
    };
    format!(
        "SET HEADING ON\nSET PAGESIZE 5000\nSET LINESIZE 4000\nSET COLSEP |\nSET FEEDBACK OFF\nSET VERIFY OFF\nSET DEFINE OFF\nSET SQLBLANKLINES ON\nSET AUTOCOMMIT OFF\nWHENEVER SQLERROR EXIT FAILURE ROLLBACK\nWHENEVER OSERROR EXIT FAILURE ROLLBACK\nCONNECT {user}/{password}@{connect}\n{executable}\nEXIT SUCCESS COMMIT\n"
    )
}

fn transaction_block(statements: &[String]) -> AppResult<String> {
    let mut block = String::from("BEGIN\n");
    for sql in statements {
        if sql.len() > 8_000 {
            return Err(AppError::msg("Oracle 网格 / CSV 事务已取消：单条变更 SQL 超过 8000 字节。SQL*Plus 的行长和 PL/SQL 字符串长度有限，无法安全编码此语句；没有执行任何变更，请使用原生客户端处理大单元格"));
        }
        // Encoding keeps arbitrary cell contents out of SQL*Plus's command
        // parser. Chunk hex below its input-line limit, then decode in Oracle.
        let hex = sql
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<String>();
        let chunks = hex
            .as_bytes()
            .chunks(1800)
            .map(|chunk| {
                format!(
                    "    '{}'",
                    std::str::from_utf8(chunk).expect("hex is ASCII")
                )
            })
            .collect::<Vec<_>>()
            .join(" ||\n");
        block.push_str(&format!(
            "  EXECUTE IMMEDIATE UTL_I18N.RAW_TO_CHAR(HEXTORAW(\n{chunks}\n  ), 'AL32UTF8');\n"
        ));
    }
    block
        .push_str("  COMMIT;\nEXCEPTION\n  WHEN OTHERS THEN\n    ROLLBACK;\n    RAISE;\nEND;\n/\n");
    Ok(block)
}

fn column_strings(outcome: &QueryOutcome) -> Vec<String> {
    outcome
        .rows
        .iter()
        .filter_map(|row| row.first())
        .map(|value| match value {
            JsonValue::String(text) => text.trim().to_string(),
            other => other.to_string(),
        })
        .filter(|text| !text.is_empty())
        .collect()
}

fn text_at(row: &[JsonValue], index: usize) -> String {
    row.get(index)
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn parse_sqlplus(text: &str) -> QueryOutcome {
    let lines = text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .filter(|line| !line.to_ascii_lowercase().starts_with("connected to"))
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return QueryOutcome {
            columns: Vec::new(),
            rows: Vec::new(),
            rows_affected: 0,
            truncated: false,
        };
    }
    let header = lines[0];
    let names = header
        .split('|')
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect::<Vec<_>>();
    let data_lines = lines.iter().skip(1).filter(|line| {
        !line
            .chars()
            .all(|ch| ch == '-' || ch == '|' || ch.is_whitespace())
    });
    let rows = data_lines
        .map(|line| {
            line.split('|')
                .map(|cell| JsonValue::String(cell.trim().to_string()))
                .take(names.len().max(1))
                .collect::<Vec<_>>()
        })
        .filter(|row| {
            row.iter()
                .any(|cell| cell.as_str().map(|text| !text.is_empty()).unwrap_or(false))
        })
        .collect::<Vec<_>>();
    let columns = names
        .iter()
        .map(|name| ColumnMeta {
            name: name.clone(),
            data_type: "varchar2".into(),
            nullable: true,
            primary_key: false,
            auto_increment: false,
            default_value: None,
        })
        .collect();
    QueryOutcome {
        columns,
        rows,
        rows_affected: 0,
        truncated: false,
    }
}

#[cfg(test)]
mod tests {
    use super::{sqlplus_script, transaction_block};

    #[test]
    fn transaction_script_uses_one_connection_and_rolls_back_any_failure() {
        let sql = "INSERT INTO items (value) VALUES ('中文 & x\n/\nEXIT\n')".to_string();
        let block = transaction_block(&[sql.clone(), "UPDATE items SET id = 2".into()]).unwrap();
        let script = sqlplus_script("user", "password", "//localhost:1521/ORCL", &block);
        assert_eq!(script.matches("CONNECT ").count(), 1);
        assert_eq!(script.matches("EXECUTE IMMEDIATE").count(), 2);
        assert_eq!(script.matches("  COMMIT;").count(), 1);
        assert!(script.contains("WHENEVER SQLERROR EXIT FAILURE ROLLBACK"));
        assert!(script.contains("WHENEVER OSERROR EXIT FAILURE ROLLBACK"));
        assert!(script.contains("EXCEPTION\n  WHEN OTHERS THEN\n    ROLLBACK;\n    RAISE;"));
        assert!(script.contains("SET AUTOCOMMIT OFF"));
        assert!(script.contains("SET DEFINE OFF"));
        assert!(script.ends_with("END;\n/\nEXIT SUCCESS COMMIT\n"));
        assert!(
            !script.contains(&sql),
            "cell text must not enter SQL*Plus's command parser"
        );
        assert!(script.lines().all(|line| line.len() < 2000));
    }

    #[test]
    fn oversized_transaction_is_rejected_before_a_process_is_started() {
        let error = transaction_block(&["INSERT INTO items VALUES (1)".into(), "x".repeat(8001)])
            .unwrap_err();
        assert!(error.to_string().contains("8000 字节"));
    }

    #[test]
    fn standalone_sql_is_terminated_so_sqlplus_executes_it() {
        let script = sqlplus_script("u", "p", "server", "SELECT 1 FROM dual");
        assert!(script.contains("SELECT 1 FROM dual;\nEXIT SUCCESS COMMIT"));
    }
}
