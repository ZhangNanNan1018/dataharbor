use crate::error::{AppError, AppResult};
use crate::models::{Engine, StoredProfile};
use crate::state::AppState;
use serde::Serialize;
use serde_json::Value as JsonValue;
use std::fs;
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

pub(super) fn read_vec<T: serde::de::DeserializeOwned>(state: &AppState, name: &str) -> Vec<T> {
    let path = state.config_dir().join(name);
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub(super) fn write_vec<T: Serialize>(state: &AppState, name: &str, items: &[T]) -> AppResult<()> {
    write_json(state, name, items)
}

pub(super) fn write_json<T: Serialize + ?Sized>(
    state: &AppState,
    name: &str,
    value: &T,
) -> AppResult<()> {
    let path = state.config_dir().join(name);
    let text = serde_json::to_string_pretty(value).map_err(|err| AppError::msg(err.to_string()))?;
    let tmp = state.config_dir().join(format!("{name}.tmp"));
    fs::write(&tmp, text).map_err(|err| AppError::msg(err.to_string()))?;
    fs::rename(&tmp, path).map_err(|err| AppError::msg(err.to_string()))
}

pub(super) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|item| item.as_secs())
        .unwrap_or(0)
}
