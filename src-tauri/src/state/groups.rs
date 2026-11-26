use super::{lock, now_string, store, AppError, AppResult, AppState, HashMap};
use crate::models::GroupEdit;
use std::collections::HashSet;

fn parts(path: &str) -> Vec<&str> {
    path.split('/')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}

impl AppState {
    pub fn edit_connection_group(
        &self,
        path: &str,
        edit: GroupEdit,
    ) -> AppResult<HashMap<String, String>> {
        let source = parts(path);
        let (_, parent) = source
            .split_last()
            .ok_or_else(|| AppError::msg("系统未分组不能修改"))?;
        let rename = match &edit {
            GroupEdit::Rename { name } => {
                let name = name.trim();
                if name.is_empty() || name.contains('/') {
                    return Err(AppError::msg("分组名称不能为空或包含斜杠"));
                }
                Some(name)
            }
            GroupEdit::Dissolve => None,
        };
        let _file_lock = self.profile_store_lock()?;
        let mut cached = lock(&self.profiles);
        let mut profiles = store::load_profiles(&self.config_dir)?;
        let mut siblings = HashSet::new();
        for profile in &profiles {
            let group = parts(&profile.group);
            if group.starts_with(parent) && !group.starts_with(&source) {
                if let Some(sibling) = group.get(parent.len()) {
                    siblings.insert(sibling.to_string());
                }
            }
        }

        let mut changes = HashMap::new();
        let now = now_string();
        for profile in &mut profiles {
            let group = parts(&profile.group);
            let Some(children) = group.strip_prefix(source.as_slice()) else {
                continue;
            };
            let dest: Vec<_> = parent
                .iter()
                .copied()
                .chain(rename)
                .chain(children.iter().copied())
                .collect();
            if let Some(name) = dest.get(parent.len()) {
                if siblings.contains(*name) {
                    return Err(AppError::msg(format!("同级已有分组“{name}”，请先重命名")));
                }
            }
            let dest = dest.join("/");
            if dest.chars().count() > 160 {
                return Err(AppError::msg("变更后的分组路径超过 160 个字符"));
            }
            changes.insert(profile.id.clone(), dest.clone());
            profile.group = dest;
            profile.updated_at.clone_from(&now);
        }
        if changes.is_empty() {
            return Err(AppError::msg("分组已不存在，请刷新连接列表"));
        }
        store::save_profiles(&self.config_dir, &profiles)?;
        *cached = profiles;
        Ok(changes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Engine, SaveConnectionRequest};
    use crate::operations::OperationContext;
    use std::{fs, sync::Arc};

    fn fixture() -> AppState {
        let dir = std::env::temp_dir().join(format!("dataharbor-groups-{}", uuid::Uuid::new_v4()));
        AppState::open_in(dir, true).unwrap()
    }

    fn add(state: &AppState, group: &str) -> String {
        state
            .save_connection(SaveConnectionRequest {
                name: "连接".into(),
                engine: Engine::Sqlite,
                file_path: state.config_dir.join("db.sqlite").display().to_string(),
                group: group.into(),
                password: Some("test-password".into()),
                ..Default::default()
            })
            .unwrap()
            .connection
            .id
    }

    fn rename(name: &str) -> GroupEdit {
        GroupEdit::Rename { name: name.into() }
    }

    #[test]
    fn nested_edits_preserve_live_sessions_secrets_and_persist_on_restart() {
        let state = fixture();
        let dir = state.config_dir.clone();
        let direct = add(&state, "翼马");
        let nested = add(&state, " 翼马 / / 测试 ");
        let deep = add(&state, "翼马/测试/内部");
        let other = add(&state, "翼马2");
        let secret = lock(&state.profiles)[0].password_enc.clone();
        state.connect(&direct).unwrap();
        let owner = state.session_owner(&direct).unwrap();
        let tab = state
            .reconnect_query_session(&direct, "main", None)
            .unwrap();
        let execute = |sql| {
            state.with_query_session(
                &direct,
                "main",
                Some(&tab),
                &OperationContext::detached(),
                |s| s.execute_affected("main", sql),
            )
        };
        execute("CREATE TEMP TABLE group_marker (id INTEGER)").unwrap();
        let changed = state.edit_connection_group("翼马", rename("客户")).unwrap();
        assert_eq!(changed.len(), 3);
        assert_eq!(changed[&nested], "客户/测试");
        assert_eq!(changed[&deep], "客户/测试/内部");
        state
            .edit_connection_group("客户/测试", GroupEdit::Dissolve)
            .unwrap();
        state
            .edit_connection_group("客户", GroupEdit::Dissolve)
            .unwrap();
        assert!(Arc::ptr_eq(&owner, &state.session_owner(&direct).unwrap()));
        assert_eq!(execute("INSERT INTO group_marker VALUES (1)").unwrap(), 1);
        drop(state);
        let reopened = AppState::open_in(dir.clone(), true).unwrap();
        let profiles = lock(&reopened.profiles);
        let paths: HashMap<_, _> = profiles.iter().map(|p| (&p.id, p.group.as_str())).collect();
        assert_eq!(
            (paths[&direct], paths[&nested], paths[&deep], paths[&other]),
            ("", "", "内部", "翼马2")
        );
        assert_eq!(profiles[0].password_enc, secret);
        drop(profiles);
        drop(reopened);
        drop(owner);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn validation_collisions_and_write_errors_leave_disk_and_cache_intact() {
        let state = fixture();
        let dir = state.config_dir.clone();
        add(&state, "A/X");
        add(&state, "A/X/Y");
        add(&state, "A/Y/deep");
        let file = dir.join("connections.json");
        let original = fs::read(&file).unwrap();
        let too_long = "长".repeat(157);
        for edit in [
            rename(" "),
            rename("a/b"),
            rename("Y"),
            rename(&too_long),
            GroupEdit::Dissolve,
        ] {
            assert!(state.edit_connection_group("A/X", edit).is_err());
        }
        assert!(state
            .edit_connection_group("", GroupEdit::Dissolve)
            .is_err());
        assert!(state
            .edit_connection_group("deleted", rename("new"))
            .is_err());
        // Prevent publishing the backup, after the complete edit has been prepared.
        let backup = dir.join("connections.json.bak");
        fs::remove_file(&backup).unwrap();
        fs::create_dir(&backup).unwrap();
        assert!(state.edit_connection_group("A", rename("B")).is_err());
        assert_eq!(fs::read(file).unwrap(), original);
        assert_eq!(state.list_connections()[1].group, "A/X/Y");
        drop(state);
        fs::remove_dir_all(dir).unwrap();
    }
}
