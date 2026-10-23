use super::storage::{now_secs, read_vec, update_vec};
use super::types::{Dashboard, DataModel, Favorite, Job, Snippet};
use crate::error::{AppError, AppResult};
use crate::models::StoredProfile;
use crate::persistence::{self, read_collection};
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use uuid::Uuid;

pub fn list_snippets(state: &AppState) -> AppResult<Vec<Snippet>> {
    read_vec(state, "snippets.json")
}

pub fn save_snippet(state: &AppState, mut snippet: Snippet) -> AppResult<Snippet> {
    if snippet.name.trim().is_empty() {
        return Err(AppError::msg("片段需要名称"));
    }
    if snippet.id.is_empty() {
        snippet.id = Uuid::new_v4().to_string();
    }
    snippet.updated_at = now_secs().to_string();
    update_vec(state, "snippets.json", |items: &mut Vec<Snippet>| {
        if let Some(slot) = items.iter_mut().find(|item| item.id == snippet.id) {
            *slot = snippet.clone();
        } else {
            items.push(snippet.clone());
        }
        Ok(snippet)
    })
}

pub fn delete_snippet(state: &AppState, id: &str) -> AppResult<()> {
    update_vec(state, "snippets.json", |items: &mut Vec<Snippet>| {
        items.retain(|item| item.id != id);
        Ok(())
    })
}

pub fn list_favorites(state: &AppState) -> AppResult<Vec<Favorite>> {
    read_vec(state, "favorites.json")
}

pub fn save_favorite(state: &AppState, mut favorite: Favorite) -> AppResult<Favorite> {
    if favorite.id.is_empty() {
        favorite.id = Uuid::new_v4().to_string();
    }
    update_vec(state, "favorites.json", |items: &mut Vec<Favorite>| {
        if let Some(existing) = items.iter().find(|item| {
            item.connection_id == favorite.connection_id
                && item.object_name == favorite.object_name
                && item.database == favorite.database
        }) {
            return Ok(existing.clone());
        }
        items.push(favorite.clone());
        Ok(favorite)
    })
}

pub fn delete_favorite(state: &AppState, id: &str) -> AppResult<()> {
    update_vec(state, "favorites.json", |items: &mut Vec<Favorite>| {
        items.retain(|item| item.id != id);
        Ok(())
    })
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectFile {
    version: u32,
    profiles: Vec<StoredProfile>,
    snippets: Vec<Snippet>,
    jobs: Vec<Job>,
    dashboards: Vec<Dashboard>,
    models: Vec<DataModel>,
    favorites: Vec<Favorite>,
}

pub fn export_project(state: &AppState, path: &str) -> AppResult<String> {
    let guard = persistence::lock(state.config_dir())?;
    let file = ProjectFile {
        version: 1,
        profiles: crate::store::load_profiles(state.config_dir())?,
        snippets: read_collection(state.config_dir(), "snippets.json")?,
        jobs: read_collection(state.config_dir(), "jobs.json")?,
        dashboards: read_collection(state.config_dir(), "dashboards.json")?,
        models: read_collection(state.config_dir(), "models.json")?,
        favorites: read_collection(state.config_dir(), "favorites.json")?,
    };
    drop(guard);
    persistence::atomic_write(
        std::path::Path::new(path),
        &serde_json::to_vec_pretty(&file).map_err(|err| AppError::msg(err.to_string()))?,
        false,
    )?;
    Ok(path.to_string())
}

pub fn import_project(state: &AppState, path: &str) -> AppResult<String> {
    let text = fs::read_to_string(path).map_err(|err| AppError::msg(err.to_string()))?;
    let file: ProjectFile = serde_json::from_str(&text)
        .map_err(|err| AppError::msg(format!("项目包无法读取: {err}")))?;
    if file.version != 1 {
        return Err(AppError::msg("不支持的项目包版本，未修改本地配置"));
    }
    let _guard = persistence::lock(state.config_dir())?;
    let (profiles, added, needs_credentials) = state.prepare_profile_import(file.profiles)?;
    let mut writes = BTreeMap::new();
    writes.insert(
        "connections.json".into(),
        serde_json::json!({ "version": 1, "profiles": profiles }),
    );
    writes.insert(
        "snippets.json".into(),
        merged::<Snippet>(state, "snippets.json", file.snippets)?,
    );
    writes.insert(
        "dashboards.json".into(),
        merged::<Dashboard>(state, "dashboards.json", file.dashboards)?,
    );
    writes.insert(
        "models.json".into(),
        merged::<DataModel>(state, "models.json", file.models)?,
    );
    writes.insert(
        "favorites.json".into(),
        merged::<Favorite>(state, "favorites.json", file.favorites)?,
    );
    let mut jobs: Vec<Job> = read_collection(state.config_dir(), "jobs.json")?;
    let (imported_jobs, skipped_jobs) = import_jobs_disabled(&mut jobs, file.jobs);
    writes.insert("jobs.json".into(), persistence::collection(&jobs)?);
    persistence::commit_project(state.config_dir(), writes)?;
    state.refresh_profiles()?;
    Ok(format!("已并入 {added} 个新连接，以及查询片段、看板、模型和收藏。其中 {needs_credentials} 个连接的加密凭据无法使用本机密钥解密，已仅导入连接资料，需要重新填写密码（原项目包未修改）。新增任务 {imported_jobs} 个，均已禁用，需检查 SQL、连接和输出路径后手动启用；跳过同 ID 任务 {skipped_jobs} 个，已有本地任务保持原样。"))
}

fn import_jobs_disabled(jobs: &mut Vec<Job>, extra: Vec<Job>) -> (usize, usize) {
    let mut added = 0;
    let mut skipped = 0;
    for mut job in extra {
        if jobs.iter().any(|existing| existing.id == job.id) {
            skipped += 1;
            continue;
        }
        job.enabled = false;
        job.revision = Uuid::new_v4().to_string();
        job.next_run_at = 0;
        job.last_message = "从项目包导入，默认禁用；请检查 SQL、连接和输出路径后手动启用。".into();
        jobs.push(job);
        added += 1;
    }
    (added, skipped)
}

fn merged<T>(state: &AppState, name: &str, extra: Vec<T>) -> AppResult<serde_json::Value>
where
    T: Serialize + serde::de::DeserializeOwned + HasId,
{
    let mut items: Vec<T> = read_collection(state.config_dir(), name)?;
    for item in extra {
        if !items
            .iter()
            .any(|existing| existing.id_of() == item.id_of())
        {
            items.push(item);
        }
    }
    persistence::collection(&items)
}

trait HasId {
    fn id_of(&self) -> &str;
}
impl HasId for Snippet {
    fn id_of(&self) -> &str {
        &self.id
    }
}
impl HasId for Job {
    fn id_of(&self) -> &str {
        &self.id
    }
}
impl HasId for Dashboard {
    fn id_of(&self) -> &str {
        &self.id
    }
}
impl HasId for DataModel {
    fn id_of(&self) -> &str {
        &self.id
    }
}
impl HasId for Favorite {
    fn id_of(&self) -> &str {
        &self.id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn corrupt_existing_store_and_future_project_version_cannot_partially_import() {
        let dir = std::env::temp_dir().join(format!("dataharbor-import-atomic-{}", Uuid::new_v4()));
        let state = AppState::open_in(dir.clone(), true).unwrap();
        let project = dir.join("input.json");
        let mut value = serde_json::json!({"version":1,"profiles":[],"snippets":[{"id":"new","name":"new","body":"SELECT 1","folder":"","updatedAt":""}],"jobs":[],"dashboards":[],"models":[],"favorites":[]});
        fs::write(&project, serde_json::to_vec(&value).unwrap()).unwrap();
        fs::write(dir.join("models.json"), "broken existing configuration").unwrap();
        assert!(import_project(&state, project.to_str().unwrap()).is_err());
        assert!(!dir.join("connections.json").exists());
        assert!(!dir.join("snippets.json").exists());
        assert_eq!(
            fs::read_to_string(dir.join("models.json")).unwrap(),
            "broken existing configuration"
        );
        fs::write(dir.join("models.json"), "[]").unwrap();
        value["version"] = serde_json::json!(2);
        fs::write(&project, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(import_project(&state, project.to_str().unwrap())
            .unwrap_err()
            .to_string()
            .contains("版本"));
        assert!(!dir.join("snippets.json").exists());
        value["version"] = serde_json::json!(1);
        fs::write(&project, serde_json::to_vec(&value).unwrap()).unwrap();
        import_project(&state, project.to_str().unwrap()).unwrap();
        assert_eq!(list_snippets(&state).unwrap()[0].id, "new");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                &fs::read(dir.join("models.json")).unwrap()
            )
            .unwrap()["version"],
            1
        );
        drop(state);
        fs::remove_dir_all(dir).unwrap();
    }
}
