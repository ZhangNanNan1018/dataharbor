use std::fs;
use std::path::Path;
use std::process::Command;

use crate::error::{AppError, AppResult};

pub fn cron_line(_interval_minutes: u64, binary: &str) -> String {
    // Cron cannot express arbitrary elapsed intervals such as 7 or 90 minutes.
    // Wake every minute; the shared runner executes only jobs whose next_run_at is due.
    let binary = format!("'{}'", binary.replace('\'', "'\"'\"'")).replace('%', "\\%");
    format!("* * * * * {binary} --run-jobs")
}

pub fn launchd_plist(label: &str, interval_secs: u64, binary: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>{label}</string>
  <key>ProgramArguments</key><array><string>{binary}</string><string>--run-jobs</string></array>
  <key>StartInterval</key><integer>{interval}</integer>
  <key>RunAtLoad</key><true/>
</dict></plist>
"#,
        interval = interval_secs.max(60),
        label = quick_xml::escape::escape(label),
        binary = quick_xml::escape::escape(binary)
    )
}

pub fn schtasks_command(task_name: &str, interval_minutes: u64, binary: &str) -> String {
    format!(
        "schtasks /Create /F /SC MINUTE /MO {} /TN \"{}\" /TR \"\\\"{}\\\" --run-jobs\"",
        interval_minutes.max(1),
        task_name,
        binary
    )
}

pub fn install_user_schedule(config_dir: &Path, _interval_minutes: u64) -> AppResult<String> {
    // Keep the IPC argument for older callers. The OS wakes the common runner;
    // each saved job, rather than the currently edited form, owns its interval.
    let interval_minutes: u64 = 1;
    let binary = std::env::current_exe().map_err(|err| AppError::msg(err.to_string()))?;
    let binary = binary.display().to_string();
    if cfg!(target_os = "macos") {
        let dir = dirs::home_dir()
            .unwrap_or_else(|| config_dir.to_path_buf())
            .join("Library/LaunchAgents");
        fs::create_dir_all(&dir).map_err(|err| AppError::msg(err.to_string()))?;
        let path = dir.join("com.dataharbor.app.jobs.plist");
        fs::write(
            &path,
            launchd_plist(
                "com.dataharbor.app.jobs",
                interval_minutes.saturating_mul(60),
                &binary,
            ),
        )
        .map_err(|err| AppError::msg(err.to_string()))?;
        let _ = Command::new("launchctl")
            .args(["unload", &path.display().to_string()])
            .status();
        let status = Command::new("launchctl")
            .args(["load", &path.display().to_string()])
            .status();
        return match status {
            Ok(code) if code.success() => Ok(format!("已写入并加载 {}", path.display())),
            _ => Ok(format!(
                "已写入 {}。请在本机执行 launchctl load。",
                path.display()
            )),
        };
    }
    if cfg!(target_os = "windows") {
        let command = schtasks_command("DataHarbor", interval_minutes, &binary);
        let script = config_dir.join("install-schedule.bat");
        fs::write(&script, &command).map_err(|err| AppError::msg(err.to_string()))?;
        return Ok(format!(
            "已写入 {}。在 Windows 上以当前用户运行该脚本即可注册任务计划；每分钟检查已保存任务的到期时间。",
            script.display()
        ));
    }
    let line = cron_line(interval_minutes, &binary);
    let path = config_dir.join("dataharbor.cron");
    fs::write(
        &path,
        format!("# DataHarbor 计划任务。安装前请合并进现有 crontab，不要覆盖其它任务。\n{line}\n"),
    )
    .map_err(|err| AppError::msg(err.to_string()))?;
    if std::env::var("DATAHARBOR_INSTALL_CRON").ok().as_deref() == Some("1") {
        if Command::new("crontab")
            .arg(&path)
            .status()
            .map(|code| code.success())
            .unwrap_or(false)
        {
            return Ok(format!(
                "已按 DATAHARBOR_INSTALL_CRON=1 安装：{line}。这会替换当前用户的 crontab。"
            ));
        }
    }
    Ok(format!(
        "已写入 {}。没有自动执行 crontab，避免覆盖已有任务。需要时把这一行合并进去：{line}",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cron_and_launchd_include_run_jobs() {
        let cron = cron_line(15, "/usr/local/bin/dataharbor");
        assert!(cron.contains("--run-jobs"));
        assert!(cron.starts_with("* * * * * "));
        let plist = launchd_plist(
            "com.dataharbor.app.jobs",
            900,
            "/Applications/DataHarbor.app",
        );
        assert!(plist.contains("--run-jobs"));
        assert!(plist.contains("<integer>900</integer>"));
        let task = schtasks_command("DataHarbor", 30, "C:\\dataharbor.exe");
        assert!(task.contains("schtasks"));
        assert!(task.contains("--run-jobs"));
    }

    #[test]
    fn arbitrary_intervals_use_due_job_polling_and_encode_special_paths() {
        for interval in [1, 7, 15, 59, 60, 90, 1440] {
            let cron = cron_line(interval, "/opt/Data Harbor/O'Reilly 100%/dataharbor");
            assert_eq!(
                cron,
                "* * * * * '/opt/Data Harbor/O'\"'\"'Reilly 100\\%/dataharbor' --run-jobs"
            );
        }
        let plist = launchd_plist(
            "com.fixture&jobs",
            420,
            "/Applications/A&B <工具>.app/dataharbor",
        );
        assert!(plist.contains("com.fixture&amp;jobs"));
        assert!(plist.contains("A&amp;B &lt;工具&gt;.app"));
        assert!(plist.contains("<integer>420</integer>"));
    }

    #[test]
    #[cfg(windows)]
    fn installer_polls_saved_jobs_instead_of_using_the_current_form_interval() {
        let directory =
            std::env::temp_dir().join(format!("dataharbor-schedule-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        install_user_schedule(&directory, 90).unwrap();
        let command = fs::read_to_string(directory.join("install-schedule.bat")).unwrap();
        assert!(command.contains("/SC MINUTE /MO 1 /TN"));
        assert!(!command.contains("/MO 90"));
        // Generating the script does not register a real operating-system task.
        fs::remove_dir_all(directory).unwrap();
    }
}
