use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{AppError, AppResult};
use crate::operations::OperationContext;

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

#[cfg(test)]
pub fn run_plan(plan: &NativePlan) -> AppResult<String> {
    run_plan_with_operation(plan, &OperationContext::detached())
}

pub fn run_plan_with_operation(
    plan: &NativePlan,
    operation: &OperationContext,
) -> AppResult<String> {
    struct SecretCleanup<'a>(Option<&'a str>);
    impl Drop for SecretCleanup<'_> {
        fn drop(&mut self) {
            if let Some(path) = self.0 {
                let _ = fs::remove_file(path);
            }
        }
    }
    let _cleanup = SecretCleanup(plan.secret_file.as_deref());
    operation.checkpoint()?;
    let program = tool_path(&plan.program)
        .ok_or_else(|| AppError::msg(format!("没有找到 {}", plan.program)))?;
    if plan.program == "psql" {
        let mut help_output = PrivateCapture::new()?;
        let mut help = Command::new(&program);
        help.arg("--help=commands")
            .stdin(Stdio::null())
            .stdout(Stdio::from(
                help_output.file().try_clone().map_err(io_error)?,
            ));
        run_process(&mut help, "psql 能力检测", operation)?;
        let text = help_output.read_bounded(256 * 1024)?;
        if !text.contains("\\restrict") {
            return Err(AppError::msg("本机 psql 不支持 restricted 安全模式，已拒绝执行外部脚本；请更新 PostgreSQL 客户端补丁版本"));
        }
    }
    let mut command = Command::new(program);
    command.args(&plan.args).stdout(Stdio::null());
    if let Some(path) = &plan.stdin_file {
        command.stdin(Stdio::from(fs::File::open(path).map_err(io_error)?));
    } else {
        command.stdin(Stdio::null());
    }
    for (key, value) in &plan.env {
        command.env(key, value);
    }
    operation.report(
        "执行原生工具",
        0,
        None,
        "取消将停止当前工具进程；还原可能已有数据库副作用，停止后需核对目标数据",
    );
    run_process(&mut command, &plan.program, operation)?;
    operation.checkpoint()?;
    Ok(plan.note.clone())
}

fn io_error(error: std::io::Error) -> AppError {
    AppError::msg(error.to_string())
}

/// Regular files avoid waiting forever for a descendant which inherited a pipe.
/// Only a bounded error prefix is read back into memory.
struct PrivateCapture {
    path: PathBuf,
    file: Option<fs::File>,
}
impl PrivateCapture {
    fn new() -> AppResult<Self> {
        let path =
            std::env::temp_dir().join(format!("dataharbor-native-{}.log", uuid::Uuid::new_v4()));
        let file = private_output(&path)?;
        Ok(Self {
            path,
            file: Some(file),
        })
    }
    fn file(&self) -> &fs::File {
        self.file.as_ref().expect("capture file is open")
    }
    fn read_bounded(&mut self, limit: u64) -> AppResult<String> {
        let file = self.file.as_mut().expect("capture file is open");
        file.seek(SeekFrom::Start(0)).map_err(io_error)?;
        let mut bytes = Vec::new();
        file.take(limit).read_to_end(&mut bytes).map_err(io_error)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}
impl Drop for PrivateCapture {
    fn drop(&mut self) {
        self.file.take();
        let _ = fs::remove_file(&self.path);
    }
}
fn private_output(path: &Path) -> AppResult<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map_err(io_error)
}

fn run_process(command: &mut Command, label: &str, operation: &OperationContext) -> AppResult<()> {
    operation.checkpoint()?;
    let mut errors = PrivateCapture::new()?;
    command.stderr(Stdio::from(errors.file().try_clone().map_err(io_error)?));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let spawned = command.spawn();
    // Release the parent's redirection handle immediately (important for
    // deleting the capture file on Windows after the child has been reaped).
    command.stderr(Stdio::null());
    let child = std::sync::Arc::new(std::sync::Mutex::new(spawned.map_err(io_error)?));
    struct Reap(std::sync::Arc<std::sync::Mutex<std::process::Child>>);
    impl Drop for Reap {
        fn drop(&mut self) {
            let mut child = self.0.lock().unwrap_or_else(|p| p.into_inner());
            if !matches!(child.try_wait(), Ok(Some(_))) {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
    }
    let _reap = Reap(child.clone());
    let signal_child = child.clone();
    let interrupt = operation.bind_interrupt(Some(std::sync::Arc::new(move || {
        let mut child = signal_child.lock().unwrap_or_else(|p| p.into_inner());
        if child.try_wait().map_err(io_error)?.is_none() {
            child.kill().map_err(io_error)?;
        }
        Ok(())
    })))?;
    let status = loop {
        operation.checkpoint()?;
        if let Some(status) = child
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .try_wait()
            .map_err(io_error)?
        {
            break status;
        }
        if errors.file().metadata().map_err(io_error)?.len() > 8 * 1024 * 1024 {
            return Err(AppError::msg(format!("{label} 错误输出过多，已停止进程")));
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    };
    drop(interrupt);
    operation.checkpoint()?;
    if !status.success() {
        let text = errors.read_bounded(4096)?;
        let text = text.trim().chars().take(400).collect::<String>();
        return Err(AppError::msg(format!("{label} 失败：{text}")));
    }
    Ok(())
}

/// Remove only a canonical, paired pg_dump restriction envelope. The process
/// installs its own unpredictable restriction key before interpreting this copy.
#[cfg(test)]
pub fn prepare_pg_script(source: &Path, destination: &Path) -> AppResult<()> {
    prepare_pg_script_with_operation(source, destination, &OperationContext::detached())
}

pub fn prepare_pg_script_with_operation(
    source: &Path,
    destination: &Path,
    operation: &OperationContext,
) -> AppResult<()> {
    operation.checkpoint()?;
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
    let mut output = private_output(destination)?;
    let result = (|| -> AppResult<()> {
        input.seek(SeekFrom::Start(0)).map_err(io_error)?;
        copy_with_operation(
            &mut (&mut input).take(skip_start),
            &mut output,
            operation,
            None,
        )?;
        input.seek(SeekFrom::Start(skip_end)).map_err(io_error)?;
        copy_with_operation(
            &mut (&mut input).take(copy_end - skip_end),
            &mut output,
            operation,
            None,
        )?;
        output.flush().map_err(io_error)
    })();
    drop(output);
    if let Err(err) = result {
        let _ = fs::remove_file(destination);
        return Err(err);
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

fn copy_with_operation(
    reader: &mut impl Read,
    writer: &mut impl Write,
    operation: &OperationContext,
    total: Option<u64>,
) -> AppResult<u64> {
    let mut copied = 0;
    let mut buffer = [0; 64 * 1024];
    loop {
        operation.checkpoint()?;
        let size = reader.read(&mut buffer).map_err(io_error)?;
        if size == 0 {
            break;
        }
        writer.write_all(&buffer[..size]).map_err(io_error)?;
        copied += size as u64;
        operation.report("复制脚本（字节）", copied, total, "尚未替换目标文件");
    }
    operation.checkpoint()?;
    Ok(copied)
}

pub fn convert_dump_to_sql_with_operation(
    source: &Path,
    destination: &Path,
    operation: &OperationContext,
) -> AppResult<String> {
    operation.checkpoint()?;
    let mut input = fs::File::open(source).map_err(io_error)?;
    let length = input.metadata().map_err(io_error)?.len();
    let mut bytes = [0; 256];
    let read = input.read(&mut bytes).map_err(io_error)?;
    let bytes = &bytes[..read];
    let temporary = destination.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let _cleanup = Cleanup(temporary.clone());
    let message = if bytes.starts_with(b"PGDMP") {
        let plan = NativePlan {
            program: "pg_restore".into(),
            args: vec![
                "-f".into(),
                temporary.display().to_string(),
                source.display().to_string(),
            ],
            env: vec![],
            secret_file: None,
            stdin_file: None,
            note: "已用 pg_restore 转成 SQL".into(),
        };
        run_plan_with_operation(&plan, operation)?
    } else if bytes.starts_with(b"SQLite format 3") {
        let tool = tool_path("sqlite3").ok_or_else(|| {
            AppError::msg("这是 SQLite 文件。没有 sqlite3 命令时，请在应用里直接打开该文件。")
        })?;
        let output = private_output(&temporary)?;
        let mut command = Command::new(tool);
        command
            .arg(source)
            .arg(".dump")
            .stdin(Stdio::null())
            .stdout(Stdio::from(output));
        operation.report("转换 SQLite 备份", 0, None, "取消后不会替换原目标文件");
        run_process(&mut command, "sqlite3 .dump", operation)?;
        "已用 sqlite3 .dump 转成 SQL".into()
    } else {
        let head = String::from_utf8_lossy(bytes);
        let head = head.trim_start();
        if !["--", "CREATE", "INSERT", "/*"]
            .iter()
            .any(|prefix| head.starts_with(prefix))
        {
            return Err(AppError::msg("无法识别的备份。mongodump 归档请改用 JSON 导出；SQL Server 原生备份需在数据库服务器恢复。"));
        }
        let mut output = private_output(&temporary)?;
        input.seek(SeekFrom::Start(0)).map_err(io_error)?;
        copy_with_operation(&mut input, &mut output, operation, Some(length))?;
        output.flush().map_err(io_error)?;
        "源文件已经是 SQL，已原样复制".into()
    };
    drop(input);
    operation.checkpoint()?;
    fs::rename(&temporary, destination).map_err(io_error)?;
    Ok(message)
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

    #[test]
    fn native_child_fixture() {
        let Ok(marker) = std::env::var("DATAHARBOR_NATIVE_CHILD_MARKER") else {
            return;
        };
        let mut ticks = 0u64;
        loop {
            fs::write(&marker, ticks.to_string()).unwrap();
            eprintln!("controlled fixture heartbeat {ticks}");
            ticks += 1;
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn precancelled_native_plan_never_spawns_and_cleans_secret() {
        let directory =
            std::env::temp_dir().join(format!("dataharbor-native-cancel-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let marker = directory.join("started");
        let secret = directory.join("secret");
        write_secret(&secret, "dummy").unwrap();
        let plan = NativePlan {
            program: std::env::current_exe().unwrap().display().to_string(),
            args: vec![
                "--exact".into(),
                "native::tests::native_child_fixture".into(),
                "--nocapture".into(),
            ],
            env: vec![(
                "DATAHARBOR_NATIVE_CHILD_MARKER".into(),
                marker.display().to_string(),
            )],
            secret_file: Some(secret.display().to_string()),
            stdin_file: None,
            note: "fixture".into(),
        };
        let operation = OperationContext::detached();
        operation.request_cancel().unwrap();
        assert!(matches!(
            run_plan_with_operation(&plan, &operation),
            Err(AppError::Cancelled { .. })
        ));
        assert!(!marker.exists());
        assert!(!secret.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn running_native_child_is_killed_reaped_and_cannot_keep_writing() {
        let directory =
            std::env::temp_dir().join(format!("dataharbor-native-child-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let marker = directory.join("heartbeat");
        let plan = NativePlan {
            program: std::env::current_exe().unwrap().display().to_string(),
            args: vec![
                "--exact".into(),
                "native::tests::native_child_fixture".into(),
                "--nocapture".into(),
            ],
            env: vec![(
                "DATAHARBOR_NATIVE_CHILD_MARKER".into(),
                marker.display().to_string(),
            )],
            secret_file: None,
            stdin_file: None,
            note: "fixture".into(),
        };
        let operation = OperationContext::detached();
        let worker_operation = operation.clone();
        let worker = std::thread::spawn(move || run_plan_with_operation(&plan, &worker_operation));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !marker.exists() || !operation.snapshot().interruptible {
            assert!(
                std::time::Instant::now() < deadline,
                "native fixture did not start"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        operation.request_cancel().unwrap();
        assert!(matches!(
            worker.join().unwrap(),
            Err(AppError::Cancelled { .. })
        ));
        let after = fs::read(&marker).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(80));
        assert_eq!(fs::read(&marker).unwrap(), after);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn cancelled_conversion_keeps_original_destination() {
        let directory = std::env::temp_dir().join(format!(
            "dataharbor-convert-cancel-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&directory).unwrap();
        let source = directory.join("source.sql");
        let destination = directory.join("target.sql");
        fs::write(&source, "-- fixture\nSELECT 1;").unwrap();
        fs::write(&destination, "previous artifact").unwrap();
        let operation = OperationContext::detached();
        operation.request_cancel().unwrap();
        assert!(matches!(
            convert_dump_to_sql_with_operation(&source, &destination, &operation),
            Err(AppError::Cancelled { .. })
        ));
        assert_eq!(
            fs::read_to_string(destination).unwrap(),
            "previous artifact"
        );
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 2);
        fs::remove_dir_all(directory).unwrap();
    }
}
