use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::models::StoredProfile;

#[derive(Debug, Serialize, Deserialize)]
struct ProfileFile {
    version: u32,
    profiles: Vec<StoredProfile>,
}

pub fn load_profiles(config_dir: &Path) -> AppResult<Vec<StoredProfile>> {
    let path = config_dir.join("connections.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(&path)
        .map_err(|err| AppError::msg(format!("读取连接配置失败: {err}")))?;
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let file: ProfileFile = serde_json::from_str(&text)
        .map_err(|err| AppError::msg(format!("连接配置损坏: {err}")))?;
    Ok(file.profiles)
}

pub fn save_profiles(config_dir: &Path, profiles: &[StoredProfile]) -> AppResult<()> {
    let path = config_dir.join("connections.json");
    let file = ProfileFile {
        version: 1,
        profiles: profiles.to_vec(),
    };
    let text = serde_json::to_string_pretty(&file)
        .map_err(|err| AppError::msg(format!("序列化连接配置失败: {err}")))?;
    let tmp = config_dir.join("connections.json.tmp");
    fs::write(&tmp, text).map_err(|err| AppError::msg(format!("写入连接配置失败: {err}")))?;
    fs::rename(&tmp, &path).map_err(|err| AppError::msg(format!("写入连接配置失败: {err}")))?;
    Ok(())
}
