use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Command;

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, PartialEq)]
pub struct NativePlan {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub secret_file: Option<String>,
    pub note: String,
}

pub fn tool_path(name: &str) -> Option<String> {
    let output = Command::new("sh").arg("-c").arg(format!("command -v {name}")).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

pub fn mysql_plan(host: &str, port: u16, user: &str, database: &str, out: &str, defaults_file: &str) -> NativePlan {
    NativePlan {
        program: "mysqldump".into(),
        args: vec![
            format!("--defaults-extra-file={defaults_file}"),
            "--single-transaction".into(),
            "--routines".into(),
            "--databases".into(),
            database.to_string(),
            format!("--result-file={out}"),
        ],
        env: Vec::new(),
        secret_file: Some(defaults_file.into()),
        note: format!("mysqldump {user}@{host}:{port}。密码写在权限 0600 的临时文件里，不放进命令行。"),
    }
}

pub fn mysql_defaults(host: &str, port: u16, user: &str, password: &str) -> String {
    format!("[client]\nhost={host}\nport={port}\nuser={user}\npassword={password}\n")
}

pub fn pg_plan(host: &str, port: u16, user: &str, database: &str, out: &str, password: &str) -> NativePlan {
    NativePlan {
        program: "pg_dump".into(),
        args: vec![
            "-h".into(),
            host.into(),
            "-p".into(),
            port.to_string(),
            "-U".into(),
            user.into(),
            "-d".into(),
            database.into(),
            "-f".into(),
            out.into(),
            "--no-password".into(),
        ],
        env: vec![("PGPASSWORD".into(), password.into())],
        secret_file: None,
        note: "pg_dump。密码只放在子进程环境变量 PGPASSWORD，不写进参数。".into(),
    }
}

pub fn mongo_plan(host: &str, port: u16, user: &str, password: &str, database: &str, out: &str) -> NativePlan {
    let mut args = vec!["--host".into(), host.into(), "--port".into(), port.to_string(), format!("--archive={out}")];
    if !user.is_empty() {
        args.push("--username".into());
        args.push(user.into());
        args.push("--password".into());
        args.push(password.into());
        args.push("--authenticationDatabase".into());
        args.push("admin".into());
    }
    if !database.is_empty() {
        args.push("--db".into());
        args.push(database.into());
    }
    NativePlan {
        program: "mongodump".into(),
        args,
        env: Vec::new(),
        secret_file: None,
        note: "mongodump。密码会出现在子进程参数中，这是该工具的限制，日志里不会打印整条命令。".into(),
    }
}

pub fn mssql_backup_sql(database: &str, path: &str) -> String {
    let db = database.replace(']', "]]");
    let path = path.replace('\'', "''");
    format!("BACKUP DATABASE [{db}] TO DISK = N'{path}' WITH FORMAT, INIT, NAME = N'dataharbor'")
}

pub fn mssql_restore_sql(database: &str, path: &str) -> String {
    let db = database.replace(']', "]]");
    let path = path.replace('\'', "''");
    format!("RESTORE DATABASE [{db}] FROM DISK = N'{path}' WITH REPLACE")
}

pub fn write_secret(path: &Path, body: &str) -> AppResult<()> {
    let mut file = fs::File::create(path).map_err(|err| AppError::msg(err.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    file.write_all(body.as_bytes()).map_err(|err| AppError::msg(err.to_string()))?;
    Ok(())
}

pub fn run_plan(plan: &NativePlan) -> AppResult<String> {
    if tool_path(&plan.program).is_none() {
        return Err(AppError::msg(format!("没有找到 {}", plan.program)));
    }
    let mut command = Command::new(&plan.program);
    command.args(&plan.args);
    for (key, value) in &plan.env {
        command.env(key, value);
    }
    let output = command.output().map_err(|err| AppError::msg(err.to_string()))?;
    if let Some(path) = &plan.secret_file {
        let _ = fs::remove_file(path);
    }
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let text = err.trim();
        let text = if text.len() > 400 { &text[..400] } else { text };
        return Err(AppError::msg(format!("{} 失败：{text}", plan.program)));
    }
    Ok(plan.note.clone())
}

pub fn convert_dump_to_sql(source: &Path, destination: &Path) -> AppResult<String> {
    let bytes = fs::read(source).map_err(|err| AppError::msg(err.to_string()))?;
    if bytes.starts_with(b"PGDMP") {
        if tool_path("pg_restore").is_none() {
            return Err(AppError::msg("这是 PostgreSQL 自定义格式备份，需要本机 pg_restore 才能转成 SQL。"));
        }
        let status = Command::new("pg_restore")
            .args(["-f", &destination.display().to_string(), &source.display().to_string()])
            .status()
            .map_err(|err| AppError::msg(err.to_string()))?;
        if !status.success() {
            return Err(AppError::msg("pg_restore 没有写出 SQL"));
        }
        return Ok("已用 pg_restore 转成 SQL".into());
    }
    if bytes.starts_with(b"SQLite format 3") {
        if tool_path("sqlite3").is_none() {
            return Err(AppError::msg("这是 SQLite 文件。没有 sqlite3 命令时，请在应用里直接打开该文件。"));
        }
        let output = Command::new("sqlite3")
            .arg(source)
            .arg(".dump")
            .output()
            .map_err(|err| AppError::msg(err.to_string()))?;
        if !output.status.success() {
            return Err(AppError::msg("sqlite3 .dump 失败"));
        }
        fs::write(destination, &output.stdout).map_err(|err| AppError::msg(err.to_string()))?;
        return Ok("已用 sqlite3 .dump 转成 SQL".into());
    }
    let text = String::from_utf8_lossy(&bytes);
    let head = text.trim_start();
    if head.starts_with("--") || head.starts_with("CREATE") || head.starts_with("INSERT") || head.starts_with("/*") {
        fs::write(destination, &bytes).map_err(|err| AppError::msg(err.to_string()))?;
        return Ok("源文件已经是 SQL，已原样复制".into());
    }
    Err(AppError::msg(
        "无法识别的备份。mongodump 归档请改用 JSON 导出，或安装 bsondump 后再转换。SQL Server 的原生备份留在服务器磁盘上，不能在客户端直接展开。",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mysql_plan_keeps_password_out_of_args() {
        let plan = mysql_plan("db.internal", 3306, "root", "shop", "/tmp/a.sql", "/tmp/secret.cnf");
        let joined = plan.args.join(" ");
        assert!(!joined.contains("p@ss"));
        assert!(joined.contains("--defaults-extra-file=/tmp/secret.cnf"));
        let defaults = mysql_defaults("db.internal", 3306, "root", "p@ss");
        assert!(defaults.contains("password=p@ss"));
        assert!(!plan.note.contains("p@ss"));
    }

    #[test]
    fn pg_plan_uses_env_not_args() {
        let plan = pg_plan("localhost", 5432, "postgres", "shop", "/tmp/a.sql", "hidden");
        assert!(plan.args.iter().all(|arg| arg != "hidden"));
        assert_eq!(plan.env, vec![("PGPASSWORD".into(), "hidden".into())]);
    }

    #[test]
    fn mssql_backup_quotes_names() {
        let sql = mssql_backup_sql("my]db", "C:\\bak\\a'.bak");
        assert!(sql.contains("[my]]db]"));
        assert!(sql.contains("a''.bak"));
        assert!(sql.starts_with("BACKUP DATABASE"));
    }
}
