use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::models::StoredProfile;

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ProfileFile {
    pub version: u32,
    pub profiles: Vec<StoredProfile>,
}

pub fn load_profiles(config_dir: &Path) -> AppResult<Vec<StoredProfile>> {
    let path = config_dir.join("connections.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(&path)
        .map_err(|err| AppError::msg(format!("读取连接配置失败: {err}")))?;
    let file: ProfileFile = serde_json::from_str(&text)
        .map_err(|err| AppError::msg(format!("连接配置损坏: {err}")))?;
    if file.version != 1 { return Err(AppError::msg("不支持的连接配置版本，原文件已保留")); }
    Ok(file.profiles)
}

pub fn save_profiles(config_dir: &Path, profiles: &[StoredProfile]) -> AppResult<()> {
    let path = config_dir.join("connections.json");
    let file = ProfileFile {
        version: 1,
        profiles: profiles.to_vec(),
    };
    crate::persistence::write_json(&path, &file)
}
