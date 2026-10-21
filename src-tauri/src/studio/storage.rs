use crate::error::{AppError, AppResult};
use crate::models::{Engine, StoredProfile};
use crate::state::AppState;
use serde::Serialize;
use serde_json::Value as JsonValue;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn profile_of(state: &AppState, id: &str) -> AppResult<StoredProfile> {
    state
        .profiles_snapshot()
        .into_iter()
        .find(|profile| profile.id == id)
        .ok_or_else(|| AppError::msg("连接不存在"))
}

pub(super) fn engine_of(state: &AppState, id: &str) -> AppResult<Engine> {
    state
        .profiles_snapshot()
        .into_iter()
        .find(|profile| profile.id == id)
        .map(|profile| profile.engine)
        .ok_or_else(|| AppError::msg("连接不存在"))
}

pub(super) fn cell_string(value: Option<&JsonValue>) -> String {
    match value {
        Some(JsonValue::String(text)) => text.clone(),
        Some(JsonValue::Number(number)) => number.to_string(),
        Some(JsonValue::Bool(flag)) => flag.to_string(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

pub(super) fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub(super) fn read_vec<T: serde::de::DeserializeOwned>(
    state: &AppState,
    name: &str,
) -> AppResult<Vec<T>> {
    let path = state.config_dir().join(name);
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(AppError::msg(format!(
                "无法读取 {name}，原配置已保留：{err}"
            )))
        }
    };
    serde_json::from_str(&text)
        .map_err(|err| AppError::msg(format!("{name} 无法解析，已停止更新以保留原文件：{err}")))
}

pub(super) fn store_lock(state: &AppState, name: &str) -> AppResult<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(state.config_dir().join(format!("{name}.lock")))
        .map_err(|err| AppError::msg(format!("无法锁定 {name}：{err}")))?;
    file.lock()
        .map_err(|err| AppError::msg(format!("无法锁定 {name}：{err}")))?;
    Ok(file)
}

pub(super) fn update_vec<T, R>(
    state: &AppState,
    name: &str,
    update: impl FnOnce(&mut Vec<T>) -> AppResult<R>,
) -> AppResult<R>
where
    T: Serialize + serde::de::DeserializeOwned,
{
    let _lock = store_lock(state, name)?;
    let mut items = read_vec(state, name)?;
    let result = update(&mut items)?;
    write_json_unlocked(state, name, &items)?;
    Ok(result)
}

#[cfg(test)]
pub(super) fn write_vec<T: Serialize>(state: &AppState, name: &str, items: &[T]) -> AppResult<()> {
    write_json(state, name, items)
}

#[cfg(test)]
pub(super) fn write_json<T: Serialize + ?Sized>(
    state: &AppState,
    name: &str,
    value: &T,
) -> AppResult<()> {
    let _lock = store_lock(state, name)?;
    write_json_unlocked(state, name, value)
}

pub(super) fn write_json_unlocked<T: Serialize + ?Sized>(
    state: &AppState,
    name: &str,
    value: &T,
) -> AppResult<()> {
    let path = state.config_dir().join(name);
    let text = serde_json::to_string_pretty(value).map_err(|err| AppError::msg(err.to_string()))?;
    let tmp = state
        .config_dir()
        .join(format!("{name}.{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        if path.exists() {
            fs::copy(&path, state.config_dir().join(format!("{name}.bak")))?;
        }
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(|err: std::io::Error| AppError::msg(format!("保存 {name} 失败：{err}")))
}

pub(super) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|item| item.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_store_is_reported_and_never_replaced() {
        let dir =
            std::env::temp_dir().join(format!("dataharbor-store-invalid-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        {
            let state = AppState::open_in(dir.clone(), true).unwrap();
            for name in [
                "snippets.json",
                "favorites.json",
                "dashboards.json",
                "models.json",
            ] {
                fs::write(dir.join(name), "{ damaged").unwrap();
                assert!(read_vec::<serde_json::Value>(&state, name).is_err());
                assert!(update_vec(
                    &state,
                    name,
                    |_: &mut Vec<serde_json::Value>| -> AppResult<()> {
                        panic!("must not update an unreadable file")
                    }
                )
                .is_err());
                assert_eq!(fs::read_to_string(dir.join(name)).unwrap(), "{ damaged");
            }
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn concurrent_store_updates_preserve_all_items_and_previous_backup() {
        let dir =
            std::env::temp_dir().join(format!("dataharbor-store-race-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        {
            let state = AppState::open_in(dir.clone(), true).unwrap();
            std::thread::scope(|scope| {
                for number in 0u32..12 {
                    let state = &state;
                    scope.spawn(move || {
                        update_vec(state, "items.json", |items: &mut Vec<u32>| {
                            items.push(number);
                            Ok(())
                        })
                        .unwrap()
                    });
                }
            });
            let mut values: Vec<u32> = read_vec(&state, "items.json").unwrap();
            values.sort();
            assert_eq!(values, (0..12).collect::<Vec<_>>());
            let backup: Vec<u32> =
                serde_json::from_slice(&fs::read(dir.join("items.json.bak")).unwrap()).unwrap();
            assert_eq!(backup.len(), 11);
        }
        fs::remove_dir_all(dir).unwrap();
    }
}
