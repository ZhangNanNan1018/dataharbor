use std::fs;
use std::path::{Path, PathBuf};

use crate::crypto;
use crate::error::{AppError, AppResult};

pub const CONFIG_DIR_NAME: &str = "dataharbor";
const LEGACY_CONFIG_DIR_NAME: &str = "xiaobiti-db";
const BUNDLE_ID: &str = "com.dataharbor.app";
const LEGACY_BUNDLE_ID: &str = "com.xiaobiti.db";
const LEGACY_KEYRING_SERVICE: &str = "xiaobiti-db";
const KEYRING_SERVICE: &str = "dataharbor";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeMove {
    Absent,
    Renamed,
    Copied,
    Merged,
}

pub fn prepare_local_data(config_base: &Path) -> AppResult<()> {
    let config = migrate_tree(
        &config_base.join(LEGACY_CONFIG_DIR_NAME),
        &config_base.join(CONFIG_DIR_NAME),
    )?;
    note("配置", config);
    migrate_named_dir(dirs::data_local_dir(), "本机数据")?;
    if dirs::data_dir() != dirs::data_local_dir() {
        migrate_named_dir(dirs::data_dir(), "数据")?;
    }
    migrate_named_dir(dirs::cache_dir(), "缓存")?;
    if let Some(home) = dirs::home_dir() {
        migrate_tree(
            &home.join("Library/WebKit").join(LEGACY_BUNDLE_ID),
            &home.join("Library/WebKit").join(BUNDLE_ID),
        )?;
        migrate_tree(
            &home.join("Library/HTTPStorages").join(LEGACY_BUNDLE_ID),
            &home.join("Library/HTTPStorages").join(BUNDLE_ID),
        )?;
        migrate_tree(
            &home.join("Library/Logs").join(LEGACY_BUNDLE_ID),
            &home.join("Library/Logs").join(BUNDLE_ID),
        )?;
    }
    // A keyring miss must not block startup. The moved master.key still decrypts.
    let _ = crypto::migrate_keyring(LEGACY_KEYRING_SERVICE, KEYRING_SERVICE);
    Ok(())
}

fn migrate_named_dir(base: Option<PathBuf>, label: &str) -> AppResult<()> {
    let Some(base) = base else {
        return Ok(());
    };
    let moved = migrate_tree(&base.join(LEGACY_BUNDLE_ID), &base.join(BUNDLE_ID))?;
    note(label, moved);
    Ok(())
}

fn note(label: &str, moved: TreeMove) {
    let how = match moved {
        TreeMove::Absent => return,
        TreeMove::Renamed => "已移动",
        TreeMove::Copied => "已复制",
        TreeMove::Merged => "已补齐缺失文件",
    };
    eprintln!("DataHarbor {label}{how}");
}

pub fn migrate_tree(old: &Path, new: &Path) -> AppResult<TreeMove> {
    if !old.exists() {
        return Ok(TreeMove::Absent);
    }
    if old == new {
        return Ok(TreeMove::Absent);
    }
    if !old.is_dir() {
        return Err(AppError::msg(format!(
            "迁移本地数据失败: {} 不是目录",
            old.display()
        )));
    }
    if !new.exists() {
        if let Some(parent) = new.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .map_err(|err| AppError::msg(format!("迁移本地数据失败: {err}")))?;
            }
        }
        match fs::rename(old, new) {
            Ok(()) => return Ok(TreeMove::Renamed),
            Err(_) => {
                copy_missing(old, new)?;
                return Ok(TreeMove::Copied);
            }
        }
    }
    if !new.is_dir() {
        return Err(AppError::msg(format!(
            "迁移本地数据失败: {} 已存在且不是目录",
            new.display()
        )));
    }
    copy_missing(old, new)?;
    Ok(TreeMove::Merged)
}

fn copy_missing(old: &Path, new: &Path) -> AppResult<()> {
    fs::create_dir_all(new).map_err(|err| AppError::msg(format!("迁移本地数据失败: {err}")))?;
    let entries = fs::read_dir(old).map_err(|err| AppError::msg(format!("迁移本地数据失败: {err}")))?;
    for entry in entries {
        let entry = entry.map_err(|err| AppError::msg(format!("迁移本地数据失败: {err}")))?;
        let from = entry.path();
        let to = new.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|err| AppError::msg(format!("迁移本地数据失败: {err}")))?;
        if file_type.is_dir() {
            copy_missing(&from, &to)?;
        } else if !to.exists() {
            fs::copy(&from, &to).map_err(|err| AppError::msg(format!("迁移本地数据失败: {err}")))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::CredentialCipher;
    use uuid::Uuid;

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dataharbor-migrate-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rename_moves_tree_and_ciphertext_still_decrypts() {
        let root = scratch();
        let old = root.join(LEGACY_CONFIG_DIR_NAME);
        let new = root.join(CONFIG_DIR_NAME);
        fs::create_dir_all(old.join("samples")).unwrap();
        let key = [9u8; 32];
        fs::write(old.join("master.key"), key).unwrap();
        let secret = CredentialCipher::from_key(key).encrypt("书店密码").unwrap();
        fs::write(old.join("connections.json"), &secret).unwrap();
        fs::write(old.join("samples/bookstore.sqlite"), b"sqlite-bytes").unwrap();

        assert_eq!(migrate_tree(&old, &new).unwrap(), TreeMove::Renamed);
        assert!(!old.exists());
        assert_eq!(fs::read(new.join("connections.json")).unwrap(), secret.as_bytes());
        assert_eq!(fs::read(new.join("samples/bookstore.sqlite")).unwrap(), b"sqlite-bytes");
        let moved = fs::read(new.join("master.key")).unwrap();
        let moved_key: [u8; 32] = moved.as_slice().try_into().unwrap();
        assert_eq!(
            CredentialCipher::from_key(moved_key).decrypt(&secret).unwrap(),
            "书店密码"
        );
        assert_eq!(migrate_tree(&old, &new).unwrap(), TreeMove::Absent);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn merge_keeps_existing_files_and_copies_missing_ones() {
        let root = scratch();
        let old = root.join("old");
        let new = root.join("new");
        fs::create_dir_all(old.join("jobs")).unwrap();
        fs::create_dir_all(&new).unwrap();
        fs::write(old.join("connections.json"), b"old").unwrap();
        fs::write(new.join("connections.json"), b"new").unwrap();
        fs::write(old.join("jobs/night.json"), b"job").unwrap();

        assert_eq!(migrate_tree(&old, &new).unwrap(), TreeMove::Merged);
        assert_eq!(fs::read(new.join("connections.json")).unwrap(), b"new");
        assert_eq!(fs::read(new.join("jobs/night.json")).unwrap(), b"job");
        assert!(old.join("connections.json").exists());
        let _ = fs::remove_dir_all(root);
    }
}
