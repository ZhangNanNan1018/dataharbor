//! One interprocess configuration lock and durable file publication. Project
//! imports use a redo record: readers recover every file before observing data.
use crate::error::{AppError, AppResult};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::Path,
};

const JOURNAL: &str = "project-commit.json";
const PROJECT_FILES: &[&str] = &[
    "connections.json",
    "snippets.json",
    "jobs.json",
    "dashboards.json",
    "models.json",
    "favorites.json",
];

pub(crate) fn lock(dir: &Path) -> AppResult<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("configuration.lock"))
        .map_err(io_error)?;
    file.lock().map_err(io_error)?;
    recover(dir)?;
    Ok(file)
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8], backup: bool) -> AppResult<()> {
    let parent = path
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| AppError::msg("保存路径缺少文件名"))?
        .to_string_lossy();
    let temporary = parent.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> AppResult<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).map_err(io_error)?;
        file.write_all(bytes).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        drop(file);
        if backup && path.exists() {
            let previous = fs::read(path).map_err(io_error)?;
            atomic_write(&parent.join(format!("{name}.bak")), &previous, false)?;
        }
        fs::rename(&temporary, path).map_err(io_error)?;
        sync_directory(parent)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

pub(crate) fn write_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> AppResult<()> {
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|error| AppError::msg(error.to_string()))?;
    atomic_write(path, &bytes, true)
}

/// Legacy arrays remain readable; the first successful edit writes version 1.
pub(crate) fn read_collection<T: DeserializeOwned>(dir: &Path, name: &str) -> AppResult<Vec<T>> {
    let bytes = match fs::read(dir.join(name)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_error(error)),
    };
    decode_collection(&bytes)
        .map_err(|error| AppError::msg(format!("{name} 无法读取，原文件已保留：{error}")))
}

pub(crate) fn decode_collection<T: DeserializeOwned>(bytes: &[u8]) -> AppResult<Vec<T>> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|error| AppError::msg(error.to_string()))?;
    let items = if value.is_array() {
        value
    } else {
        if value.get("version").and_then(|version| version.as_u64()) != Some(1) {
            return Err(AppError::msg("不支持的配置版本，请使用兼容版本打开"));
        }
        value
            .get("items")
            .cloned()
            .ok_or_else(|| AppError::msg("配置缺少 items"))?
    };
    serde_json::from_value(items).map_err(|error| AppError::msg(error.to_string()))
}

pub(crate) fn collection<T: Serialize + ?Sized>(items: &T) -> AppResult<serde_json::Value> {
    serde_json::to_value(items)
        .map(|items| serde_json::json!({"version": 1, "items": items}))
        .map_err(|error| AppError::msg(error.to_string()))
}

#[derive(Serialize, Deserialize)]
struct ProjectCommit {
    version: u32,
    writes: BTreeMap<String, serde_json::Value>,
}

/// The caller holds configuration.lock. No target changes before the entire
/// transaction has been validated and its redo record durably published.
pub(crate) fn commit_project(
    dir: &Path,
    writes: BTreeMap<String, serde_json::Value>,
) -> AppResult<()> {
    let record = ProjectCommit { version: 1, writes };
    validate_record(dir, &record)?;
    let bytes = serde_json::to_vec(&record).map_err(|error| AppError::msg(error.to_string()))?;
    atomic_write(&dir.join(JOURNAL), &bytes, false)?;
    recover(dir).map_err(|error| {
        AppError::msg(format!(
            "项目导入已暂存，恢复完成前配置访问已停止，请修复文件访问后重试：{error}"
        ))
    })
}

fn validate_record(dir: &Path, record: &ProjectCommit) -> AppResult<()> {
    if record.version != 1
        || record.writes.len() != PROJECT_FILES.len()
        || PROJECT_FILES
            .iter()
            .any(|name| !record.writes.contains_key(*name))
    {
        return Err(AppError::msg("项目提交记录的版本或文件列表无效"));
    }
    for name in PROJECT_FILES {
        if dir.join(name).exists() && !dir.join(name).is_file() {
            return Err(AppError::msg(format!("配置目标不是普通文件：{name}")));
        }
    }
    Ok(())
}

fn recover(dir: &Path) -> AppResult<()> {
    let path = dir.join(JOURNAL);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_error(error)),
    };
    let record: ProjectCommit = serde_json::from_slice(&bytes)
        .map_err(|error| AppError::msg(format!("项目恢复记录损坏，已停止配置写入：{error}")))?;
    validate_record(dir, &record)?;
    for (name, value) in record.writes {
        write_json(&dir.join(name), &value)?;
    }
    fs::remove_file(path).map_err(io_error)?;
    sync_directory(dir)
}

fn sync_directory(dir: &Path) -> AppResult<()> {
    #[cfg(unix)]
    {
        File::open(dir)
            .and_then(|file| file.sync_all())
            .map_err(io_error)?;
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
    Ok(())
}
fn io_error(error: std::io::Error) -> AppError {
    AppError::msg(format!("本地存储失败：{error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_collection_upgrades_and_future_version_is_refused() {
        assert_eq!(decode_collection::<u32>(b"[1,2]").unwrap(), vec![1, 2]);
        assert_eq!(
            decode_collection::<u32>(&serde_json::to_vec(&collection(&vec![3]).unwrap()).unwrap())
                .unwrap(),
            vec![3]
        );
        assert!(decode_collection::<u32>(br#"{"version":2,"items":[]}"#).is_err());
        assert!(decode_collection::<u32>(b"").is_err());
    }
    #[test]
    fn interrupted_project_commit_recovers_all_files_before_reading() {
        let dir =
            std::env::temp_dir().join(format!("dataharbor-recovery-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        let writes = PROJECT_FILES
            .iter()
            .map(|name| {
                (
                    name.to_string(),
                    serde_json::json!({"version":1,"items":["new"]}),
                )
            })
            .collect();
        let record = ProjectCommit { version: 1, writes };
        fs::write(dir.join(JOURNAL), serde_json::to_vec(&record).unwrap()).unwrap();
        fs::write(dir.join("jobs.json"), b"[]").unwrap();
        {
            let _guard = lock(&dir).unwrap();
            for name in PROJECT_FILES {
                assert_eq!(read_collection::<String>(&dir, name).unwrap(), vec!["new"]);
            }
            assert!(!dir.join(JOURNAL).exists());
        }
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn invalid_project_targets_fail_before_any_existing_file_changes() {
        let dir =
            std::env::temp_dir().join(format!("dataharbor-preflight-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("connections.json"), b"previous").unwrap();
        fs::create_dir(dir.join("models.json")).unwrap();
        let writes = PROJECT_FILES
            .iter()
            .map(|name| (name.to_string(), serde_json::json!([])))
            .collect();
        assert!(commit_project(&dir, writes).is_err());
        assert_eq!(fs::read(dir.join("connections.json")).unwrap(), b"previous");
        assert!(!dir.join(JOURNAL).exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
