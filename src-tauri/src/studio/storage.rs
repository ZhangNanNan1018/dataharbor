use crate::error::{AppError, AppResult};
use crate::models::{Engine, StoredProfile};
use crate::state::AppState;
use serde::Serialize;
use serde_json::Value as JsonValue;
#[cfg(test)]
use std::fs;
use std::fs::File;
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
    let _guard = store_lock(state, name)?;
    crate::persistence::read_collection(state.config_dir(), name)
}

pub(super) fn store_lock(state: &AppState, _name: &str) -> AppResult<File> {
    crate::persistence::lock(state.config_dir())
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
    let mut items = crate::persistence::read_collection(state.config_dir(), name)?;
    let result = update(&mut items)?;
    write_json_unlocked(state, name, &crate::persistence::collection(&items)?)?;
    Ok(result)
}

#[cfg(test)]
pub(super) fn write_vec<T: Serialize>(state: &AppState, name: &str, items: &[T]) -> AppResult<()> {
    write_json(state, name, &crate::persistence::collection(items)?)
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
    crate::persistence::write_json(&state.config_dir().join(name), value)
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
            let backup: Vec<u32> = crate::persistence::decode_collection(
                &fs::read(dir.join("items.json.bak")).unwrap(),
            )
            .unwrap();
            assert_eq!(backup.len(), 11);
        }
        fs::remove_dir_all(dir).unwrap();
    }
}
