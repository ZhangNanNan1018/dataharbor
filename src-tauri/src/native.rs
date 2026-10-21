use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, PartialEq)]
pub struct NativePlan {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub secret_file: Option<String>,
    pub stdin_file: Option<String>,
    pub note: String,
}

pub fn tool_path(name: &str) -> Option<String> {
    let paths = std::env::var_os("PATH")?;
    let extensions = if cfg!(windows) {
        std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
    } else {
        String::new()
    };
    find_tool(name, std::env::split_paths(&paths), &extensions)
        .map(|path| path.to_string_lossy().into_owned())
}

fn find_tool(
    name: &str,
    paths: impl Iterator<Item = PathBuf>,
    extensions: &str,
) -> Option<PathBuf> {
    // Resolve an executable directly; do not interpret PATH values or tool names as shell code.
    let mut names = vec![name.to_string()];
    if Path::new(name).extension().is_none() {
        names.extend(
            extensions
                .split(';')
                .filter(|ext| !ext.is_empty())
                .map(|ext| format!("{name}{ext}")),
        );
    }
    paths
        .filter(|path| !path.as_os_str().is_empty())
        .flat_map(|path| names.iter().map(move |name| path.join(name)))
        .find(|path| executable_file(path))
}

fn executable_file(path: &Path) -> bool {
    let Ok(meta) = path.metadata() else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub fn mysql_plan(
    host: &str,
    port: u16,
    user: &str,
    database: &str,
    out: &str,
    defaults_file: &str,
) -> NativePlan {
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
        stdin_file: None,
        note: format!("mysqldump {user}@{host}:{port}。密码通过临时配置文件传入，不放进命令行。"),
    }
}

pub fn mysql_defaults(host: &str, port: u16, user: &str, password: &str) -> String {
    fn quoted(value: &str) -> String {
        format!(
            "\"{}\"",
            value
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
                .replace('\r', "\\r")
        )
    }
    format!(
        "[client]\nhost={}\nport={port}\nuser={}\npassword={}\n",
        quoted(host),
        quoted(user),
        quoted(password)
    )
}

pub fn pg_plan(
    host: &str,
    port: u16,
    user: &str,
    database: &str,
    out: &str,
    password: &str,
) -> NativePlan {
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
        env: vec![
            ("PGPASSWORD".into(), password.into()),
            ("PGSSLMODE".into(), "disable".into()),
        ],
        secret_file: None,
        stdin_file: None,
        note: "pg_dump。密码只放在子进程环境变量 PGPASSWORD，不写进参数。".into(),
    }
}

pub fn mongo_plan(
    host: &str,
    port: u16,
    user: &str,
    database: &str,
    out: &str,
    config_file: &str,
    restore: bool,
) -> NativePlan {
    let mut args = vec![
        "--config".into(),
        config_file.into(),
        "--host".into(),
        host.into(),
        "--port".into(),
        port.to_string(),
        format!("--archive={out}"),
    ];
    if !user.is_empty() {
        args.push("--username".into());
        args.push(user.into());
        args.push("--authenticationDatabase".into());
        args.push("admin".into());
    }
    if !database.is_empty() && !restore {
        args.push("--db".into());
        args.push(database.into());
    }
    NativePlan {
        program: if restore { "mongorestore" } else { "mongodump" }.into(),
        args,
        env: Vec::new(),
        secret_file: Some(config_file.into()),
        stdin_file: None,
        note: "MongoDB 原生工具已完成；密码通过临时配置文件传入。".into(),
    }
}

pub fn pg_restore_plan(
    host: &str,
    port: u16,
    user: &str,
    database: &str,
    source: &str,
    password: &str,
    custom: bool,
) -> NativePlan {
    let mut plan = pg_plan(host, port, user, database, source, password);
    plan.program = if custom { "pg_restore" } else { "psql" }.into();
    plan.args = vec![
        "-h".into(),
        host.into(),
        "-p".into(),
        port.to_string(),
        "-U".into(),
        user.into(),
        "-d".into(),
        database.into(),
        "--no-password".into(),
    ];
    if custom {
        plan.args.extend(["--exit-on-error".into(), source.into()]);
    } else {
        plan.args.extend([
            "-X".into(),
            "--set=ON_ERROR_STOP=1".into(),
            "--command".into(),
            format!("\\restrict {}", uuid::Uuid::new_v4().simple()),
            "--file".into(),
            source.into(),
        ]);
    }
    plan.note = "PostgreSQL 原生恢复已完成".into();
    plan
}

pub fn mysql_restore_plan(defaults_file: &str, database: &str, source: &str) -> NativePlan {
    NativePlan {
        program: "mysql".into(),
        args: vec![
            format!("--defaults-extra-file={defaults_file}"),
            "--binary-mode".into(),
            format!("--database={database}"),
        ],
        env: Vec::new(),
        secret_file: Some(defaults_file.into()),
        stdin_file: Some(source.into()),
        note: "MySQL 原生恢复已完成".into(),
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
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|err| AppError::msg(err.to_string()))?;
    if let Err(err) = file.write_all(body.as_bytes()) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(AppError::msg(err.to_string()));
    }
    Ok(())
}

pub fn run_plan(plan: &NativePlan) -> AppResult<String> {
    struct SecretCleanup<'a>(Option<&'a str>);
    impl Drop for SecretCleanup<'_> {
        fn drop(&mut self) {
            if let Some(path) = self.0 {
                let _ = fs::remove_file(path);
            }
        }
    }
    let _cleanup = SecretCleanup(plan.secret_file.as_deref());
    let program = tool_path(&plan.program)
        .ok_or_else(|| AppError::msg(format!("没有找到 {}", plan.program)))?;
    if plan.program == "psql" {
        let help = Command::new(&program)
            .arg("--help=commands")
            .stdin(Stdio::null())
            .output()
            .map_err(|err| AppError::msg(err.to_string()))?;
        if !help.status.success() || !String::from_utf8_lossy(&help.stdout).contains("\\restrict") {
            return Err(AppError::msg("本机 psql 不支持 restricted 安全模式，已拒绝执行外部脚本；请更新 PostgreSQL 客户端补丁版本"));
        }
    }
    let mut command = Command::new(program);
    command.args(&plan.args);
    command.stdout(Stdio::null());
    if let Some(path) = &plan.stdin_file {
        command.stdin(Stdio::from(
            fs::File::open(path).map_err(|err| AppError::msg(err.to_string()))?,
        ));
    } else {
        command.stdin(Stdio::null());
    }
    for (key, value) in &plan.env {
        command.env(key, value);
    }
    let output = command
        .output()
        .map_err(|err| AppError::msg(err.to_string()))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let text = err.trim();
        let text: String = text.chars().take(400).collect();
        return Err(AppError::msg(format!("{} 失败：{text}", plan.program)));
    }
    Ok(plan.note.clone())
}

/// Remove only a canonical, paired pg_dump restriction envelope. The process
/// installs its own unpredictable restriction key before interpreting this copy.
pub fn prepare_pg_script(source: &Path, destination: &Path) -> AppResult<()> {
    let mut input = fs::File::open(source).map_err(|err| AppError::msg(err.to_string()))?;
    let length = input
        .metadata()
        .map_err(|err| AppError::msg(err.to_string()))?
        .len();
    let mut header = vec![0; length.min(64 * 1024) as usize];
    input
        .read_exact(&mut header)
        .map_err(|err| AppError::msg(err.to_string()))?;
    let envelope = pg_restrict_header(&header);
    let (skip_start, skip_end, copy_end) = if let Some((start, end, key)) = envelope {
        let tail_start = length.saturating_sub(64 * 1024);
        input
            .seek(SeekFrom::Start(tail_start))
            .map_err(|err| AppError::msg(err.to_string()))?;
        let mut tail = Vec::new();
        input
            .read_to_end(&mut tail)
            .map_err(|err| AppError::msg(err.to_string()))?;
        let trimmed = tail.trim_ascii_end();
        let line_start = trimmed
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        let footer = std::str::from_utf8(&trimmed[line_start..])
            .unwrap_or("")
            .trim();
        if footer != format!("\\unrestrict {key}") {
            return Err(AppError::msg("PostgreSQL 脚本的安全包装不完整，已拒绝恢复"));
        }
        (start as u64, end as u64, tail_start + line_start as u64)
    } else {
        (0, 0, length)
    };
    if copy_end < skip_end {
        return Err(AppError::msg("PostgreSQL 脚本的安全包装无效"));
    }
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|err| AppError::msg(err.to_string()))?;
    let result = (|| -> std::io::Result<()> {
        input.seek(SeekFrom::Start(0))?;
        std::io::copy(&mut (&mut input).take(skip_start), &mut output)?;
        input.seek(SeekFrom::Start(skip_end))?;
        std::io::copy(&mut (&mut input).take(copy_end - skip_end), &mut output)?;
        output.flush()
    })();
    drop(output);
    if let Err(err) = result {
        let _ = fs::remove_file(destination);
        return Err(AppError::msg(err.to_string()));
    }
    Ok(())
}

fn pg_restrict_header(header: &[u8]) -> Option<(usize, usize, String)> {
    let mut offset = 0;
    for line in header.split_inclusive(|byte| *byte == b'\n') {
        let text = std::str::from_utf8(line)
            .ok()?
            .trim()
            .trim_start_matches('\u{feff}');
        if !text.is_empty() && !text.starts_with("--") {
            let key = text.strip_prefix("\\restrict ")?.trim();
            return (!key.is_empty() && key.bytes().all(|byte| byte.is_ascii_alphanumeric()))
                .then(|| (offset, offset + line.len(), key.to_string()));
        }
        offset += line.len();
    }
    None
}

pub fn convert_dump_to_sql(source: &Path, destination: &Path) -> AppResult<String> {
    let mut bytes = vec![0; 256];
    let length = fs::File::open(source)
        .and_then(|mut file| file.read(&mut bytes))
        .map_err(|err| AppError::msg(err.to_string()))?;
    bytes.truncate(length);
    if bytes.starts_with(b"PGDMP") {
        if tool_path("pg_restore").is_none() {
            return Err(AppError::msg(
                "这是 PostgreSQL 自定义格式备份，需要本机 pg_restore 才能转成 SQL。",
            ));
        }
        let status = Command::new("pg_restore")
            .args([
                "-f",
                &destination.display().to_string(),
                &source.display().to_string(),
            ])
            .status()
            .map_err(|err| AppError::msg(err.to_string()))?;
        if !status.success() {
            return Err(AppError::msg("pg_restore 没有写出 SQL"));
        }
        return Ok("已用 pg_restore 转成 SQL".into());
    }
    if bytes.starts_with(b"SQLite format 3") {
        if tool_path("sqlite3").is_none() {
            return Err(AppError::msg(
                "这是 SQLite 文件。没有 sqlite3 命令时，请在应用里直接打开该文件。",
            ));
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
    if head.starts_with("--")
        || head.starts_with("CREATE")
        || head.starts_with("INSERT")
        || head.starts_with("/*")
    {
        fs::copy(source, destination).map_err(|err| AppError::msg(err.to_string()))?;
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
        let plan = mysql_plan(
            "db.internal",
            3306,
            "root",
            "shop",
            "/tmp/a.sql",
            "/tmp/secret.cnf",
        );
        let joined = plan.args.join(" ");
        assert!(!joined.contains("p@ss"));
        assert!(joined.contains("--defaults-extra-file=/tmp/secret.cnf"));
        let defaults = mysql_defaults("db.internal", 3306, "root", "p@ss");
        assert!(defaults.contains("password=\"p@ss\""));
        assert!(!plan.note.contains("p@ss"));
    }

    #[test]
    fn pg_plan_uses_env_not_args() {
        let plan = pg_plan(
            "localhost",
            5432,
            "postgres",
            "shop",
            "/tmp/a.sql",
            "hidden",
        );
        assert!(plan.args.iter().all(|arg| arg != "hidden"));
        assert!(plan.env.contains(&("PGPASSWORD".into(), "hidden".into())));
        assert!(plan.env.contains(&("PGSSLMODE".into(), "disable".into())));
    }

    #[test]
    fn mssql_backup_quotes_names() {
        let sql = mssql_backup_sql("my]db", "C:\\bak\\a'.bak");
        assert!(sql.contains("[my]]db]"));
        assert!(sql.contains("a''.bak"));
        assert!(sql.starts_with("BACKUP DATABASE"));
    }

    #[test]
    fn tool_lookup_uses_path_and_extensions_without_a_shell() {
        let directory =
            std::env::temp_dir().join(format!("dataharbor-tools-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let tool = directory.join("pg_dump.exe");
        fs::write(&tool, b"fixture").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tool, fs::Permissions::from_mode(0o700)).unwrap();
        }
        assert_eq!(
            find_tool("pg_dump", [directory.clone()].into_iter(), ".exe;.cmd"),
            Some(tool)
        );
        assert!(find_tool(
            "pg_dump;unexpected",
            [directory.clone()].into_iter(),
            ".exe"
        )
        .is_none());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn restore_plans_handle_copy_archives_and_failures() {
        let plain = pg_restore_plan(
            "localhost",
            5432,
            "user",
            "target",
            "dump.sql",
            "secret",
            false,
        );
        assert_eq!(plain.program, "psql");
        assert!(plain.args.contains(&"-X".into()));
        assert!(plain.args.contains(&"--set=ON_ERROR_STOP=1".into()));
        assert!(plain.args.contains(&"--file".into()));
        assert!(plain.args.iter().any(|arg| arg.starts_with("\\restrict ")));
        let other = pg_restore_plan(
            "localhost",
            5432,
            "user",
            "target",
            "dump.sql",
            "secret",
            false,
        );
        assert_ne!(
            plain.args.iter().find(|arg| arg.starts_with("\\restrict ")),
            other.args.iter().find(|arg| arg.starts_with("\\restrict "))
        );
        let custom = pg_restore_plan(
            "localhost",
            5432,
            "user",
            "target",
            "dump.backup",
            "secret",
            true,
        );
        assert_eq!(custom.program, "pg_restore");
        assert!(custom.args.contains(&"--exit-on-error".into()));
        let mysql = mysql_restore_plan("secret.cnf", "target", "dump.sql");
        assert_eq!(mysql.stdin_file.as_deref(), Some("dump.sql"));
        let mongo = mongo_plan(
            "localhost",
            27017,
            "user",
            "target",
            "dump.archive",
            "secret.yaml",
            true,
        );
        assert_eq!(mongo.program, "mongorestore");
        assert!(!mongo.args.contains(&"--password".into()));
        assert!(mongo.args.contains(&"--config".into()));
    }

    #[test]
    fn secret_is_removed_even_when_process_cannot_start() {
        let secret =
            std::env::temp_dir().join(format!("dataharbor-secret-{}", uuid::Uuid::new_v4()));
        write_secret(&secret, "fixture").unwrap();
        let plan = NativePlan {
            program: format!("dataharbor-missing-{}", uuid::Uuid::new_v4()),
            args: Vec::new(),
            env: Vec::new(),
            secret_file: Some(secret.display().to_string()),
            stdin_file: None,
            note: String::new(),
        };
        assert!(run_plan(&plan).is_err());
        assert!(!secret.exists());
    }

    #[test]
    fn option_file_values_cannot_add_client_options() {
        let defaults = mysql_defaults("host", 3306, "user", "secret\nssl-mode=DISABLED\\\"");
        assert_eq!(defaults.lines().count(), 5);
        assert!(defaults.contains("\\nssl-mode=DISABLED"));
    }

    #[test]
    fn pg_wrapper_removal_preserves_copy_payload_and_client_commands_for_restricted_execution() {
        let directory =
            std::env::temp_dir().join(format!("dataharbor-pg-script-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let source = directory.join("source.sql");
        let destination = directory.join("guarded.sql");
        let body = "COPY public.items FROM stdin;\n\\\\! ordinary data\n\\N\n\\.\nSELECT 1;\n\\! dangerous-client-command\n";
        fs::write(
            &source,
            format!("-- dump\n\\restrict oldKey\n{body}\\unrestrict oldKey\n"),
        )
        .unwrap();
        prepare_pg_script(&source, &destination).unwrap();
        assert_eq!(
            fs::read_to_string(&destination).unwrap(),
            format!("-- dump\n{body}")
        );
        fs::remove_file(&destination).unwrap();
        fs::write(
            &source,
            "-- dump\n\\restrict oldKey\nSELECT 1;\n\\unrestrict wrongKey\n",
        )
        .unwrap();
        assert!(prepare_pg_script(&source, &destination).is_err());
        assert!(!destination.exists());
        fs::remove_dir_all(directory).unwrap();
    }
}
